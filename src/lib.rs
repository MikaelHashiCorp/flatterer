use csv::{ReaderBuilder, WriterBuilder};
use itertools::Itertools;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fmt;
use std::fs::{create_dir_all, remove_dir_all, File};
use std::path::PathBuf;
use std::io::prelude::*;
use std::io::{ErrorKind, Error as IoError};
use std::error::Error;


#[derive(Clone, Debug)]
pub enum PathItem {
    Key(String),
    Index(usize),
}

impl fmt::Display for PathItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PathItem::Key(key) => write!(f, "{}", key),
            PathItem::Index(index) => write!(f, "{}", index),
        }
    }
}


pub struct FlatFiles {
    output_path: PathBuf,
    main_table_name: String,
    emit_obj: Vec<Vec<String>>,
    row_number: u128,
    table_rows: HashMap<String, Vec<Map<String, Value>>>,
    output_csvs: HashMap<String, csv::Writer<File>>,
    table_metadata: HashMap<String, TableMetadata>,
}

#[derive(Serialize)]
pub struct TableMetadata {
    output_fields: HashMap<String, HashMap<String, String>>,
    fields: Vec<String>,
}

impl FlatFiles {

    pub fn new (
        output_dir: String,
        force: bool,
        main_table_name: String,
        emit_obj: Vec<Vec<String>>,
        ) ->  Result<FlatFiles, IoError> {

        let output_path = PathBuf::from(output_dir.clone());
        if output_path.is_dir() {
            if force {
                remove_dir_all(&output_path)?;
            } else {
                return Err(IoError::new(ErrorKind::AlreadyExists, format!("Directory {} already exists", output_dir)))
            }
        }
        let data_path = output_path.join("data");
        create_dir_all(&data_path)?;

        let tmp_path = output_path.join("tmp");
        create_dir_all(&tmp_path)?;

        Ok(FlatFiles {
            output_path,
            main_table_name,
            emit_obj,
            row_number: 1,
            table_rows: HashMap::new(),
            output_csvs: HashMap::new(),
            table_metadata: HashMap::new(),
        })
    }

    fn handle_obj(
        &mut self,
        mut obj: Map<String, Value>,
        emit: bool,
        full_path: Vec<PathItem>,
        no_index_path: Vec<String>,
        one_to_many_full_paths: Vec<Vec<PathItem>>,
        one_to_many_no_index_paths: Vec<Vec<String>>,
    ) -> Option<Map<String, Value>> {
        let keys: Vec<_> = obj.keys().cloned().collect();
        for key in keys {
            let value = obj.get(&key).unwrap(); //key known

            match value {
                Value::Array(arr) => {
                    let mut str_count = 0;
                    let mut obj_count = 0;
                    let arr_length = arr.len();
                    for array_value in arr {
                        if array_value.is_object() {
                            obj_count += 1
                        };
                        if array_value.is_string() {
                            str_count += 1
                        };
                    }
                    if str_count == arr_length {
                        let keys: Vec<String> = arr
                            .iter()
                            .map(|val| (val.as_str().unwrap().to_string())) //value known as str
                            .collect();
                        let new_value = json!(keys.join(","));
                        obj.insert(key, new_value);
                    } else if arr_length == 0 {
                        obj.remove(&key);
                    } else if obj_count == arr_length {
                        let mut removed_array = obj.remove(&key).unwrap(); //key known
                        let my_array = removed_array.as_array_mut().unwrap(); //key known as array
                        for (i, array_value) in my_array.iter_mut().enumerate() {
                            let my_value = array_value.take();
                            if let Value::Object(my_obj) = my_value {
                                let mut new_full_path = full_path.clone();
                                new_full_path.push(PathItem::Key(key.clone()));
                                new_full_path.push(PathItem::Index(i));

                                let mut new_one_to_many_full_paths = one_to_many_full_paths.clone();
                                new_one_to_many_full_paths.push(new_full_path.clone());

                                let mut new_no_index_path = no_index_path.clone();
                                new_no_index_path.push(key.clone());

                                let mut new_one_to_many_no_index_paths =
                                    one_to_many_no_index_paths.clone();
                                new_one_to_many_no_index_paths.push(new_no_index_path.clone());

                                self.handle_obj(
                                    my_obj,
                                    true,
                                    new_full_path,
                                    new_no_index_path,
                                    new_one_to_many_full_paths,
                                    new_one_to_many_no_index_paths,
                                );
                            }
                        }
                    } else {
                        let json_value = json!(format!("{}", value));
                        obj.insert(key, json_value);
                    }
                }
                Value::Object(_) => {
                    let my_value = obj.remove(&key).unwrap(); //key known

                    let mut new_full_path = full_path.clone();
                    new_full_path.push(PathItem::Key(key.clone()));
                    let mut new_no_index_path = no_index_path.clone();
                    new_no_index_path.push(key.clone());

                    let mut emit_child = false;
                    if self
                        .emit_obj
                        .iter()
                        .any(|emit_path| emit_path == &new_no_index_path)
                    {
                        emit_child = true;
                    }

                    if let Value::Object(my_value) = my_value {
                        let new_obj = self.handle_obj(
                            my_value,
                            emit_child,
                            new_full_path,
                            new_no_index_path,
                            one_to_many_full_paths.clone(),
                            one_to_many_no_index_paths.clone(),
                        );
                        if let Some(mut my_obj) = new_obj {
                            for (new_key, new_value) in my_obj.iter_mut() {
                                obj.insert(format!("{}_{}", key, new_key), new_value.take());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if emit {
            self.process_obj(
                obj,
                no_index_path,
                one_to_many_full_paths,
                one_to_many_no_index_paths,
            );
            None
        } else {
            Some(obj)
        }
    }

    pub fn process_obj(
        &mut self,
        mut obj: Map<String, Value>,
        no_index_path: Vec<String>,
        one_to_many_full_paths: Vec<Vec<PathItem>>,
        one_to_many_no_index_paths: Vec<Vec<String>>,
    ) {
        let mut path_iter = one_to_many_full_paths
            .iter()
            .zip(one_to_many_no_index_paths)
            .peekable();

        if one_to_many_full_paths.len() == 0 {
            obj.insert(
                String::from("_link"),
                Value::String(format!("{}", self.row_number)),
            );
        }

        while let Some((full, no_index)) = path_iter.next() {
            if path_iter.peek().is_some() {
                obj.insert(
                    format!("_link_{}", no_index.iter().join("_")),
                    Value::String(format!("{}.{}", self.row_number, full.iter().join("."))),
                );
            } else {
                obj.insert(
                    String::from("_link"),
                    Value::String(format!("{}.{}", self.row_number, full.iter().join("."))),
                );
            }
        }

        obj.insert(
            format!("_link_{}", self.main_table_name),
            Value::String(format!("{}", self.row_number)),
        );

        let mut table_name = no_index_path.join("_");

        if table_name == "" {
            table_name = self.main_table_name.clone();
        }

        if !self.table_rows.contains_key(&table_name) {
            self.table_rows.insert(table_name, vec![obj]);
        } else {
            let current_list = self.table_rows.get_mut(&table_name).unwrap(); //key known
            current_list.push(obj)
        }
    }

    pub fn create_rows(&mut self) -> Result<(), csv::Error> {
        for (table, rows) in self.table_rows.iter_mut() {
            if !self.output_csvs.contains_key(table) {
                self.output_csvs.insert(
                    table.clone(),
                    WriterBuilder::new()
                        .flexible(true)
                        .from_path(self.output_path.join(format!("tmp/{}.csv", table)))?,
                );
                self.table_metadata.insert(
                    table.clone(),
                    TableMetadata {
                        fields: vec![],
                        output_fields: HashMap::new(),
                    },
                );
            }

            let table_metadata = self.table_metadata.get_mut(table).unwrap(); //key known
            let writer = self.output_csvs.get_mut(table).unwrap(); //key known

            for row in rows {
                let mut output_row = vec![];
                for field in table_metadata.fields.iter() {
                    if let Some(value) = row.remove(field) {
                        //output_row.push(value_convert(value, &table_metadata.output_fields));
                        output_row.push(value_convert(value));
                    } else {
                        output_row.push(format!(""));
                    }
                }
                for (key, value) in row {
                    table_metadata.fields.push(key.clone());
                    output_row.push(value_convert(value.take()));
                }
                writer.write_record(output_row)?;
            }
        }
        Ok(())
    }

    pub fn process_value(&mut self, value: Value) -> Result<(), csv::Error> {
        if let Value::Object(obj) = value {
            self.handle_obj(obj, true, vec![], vec![], vec![], vec![]);
            self.row_number += 1;
        }
        self.create_rows()?;
        for val in self.table_rows.values_mut() {
            val.clear();
        }
        return Ok(())
    }

    pub fn write_files(&mut self) -> Result<(), Box<dyn Error>>{
 
        let tmp_path = self.output_path.join("tmp");
        let data_path = self.output_path.join("data");

        for (table_name, output_csv) in self.output_csvs.drain() {
            let metadata = self.table_metadata.get(&table_name).unwrap(); //key known
            let mut input_csv = output_csv.into_inner()?;
            input_csv.flush()?;

            let csv_reader = ReaderBuilder::new()
                .has_headers(false)
                .flexible(true)
                .from_path(tmp_path.join(format!("{}.csv", table_name)))?;

            let mut csv_writer = WriterBuilder::new()
                .from_path(data_path.join(format!("{}.csv", table_name)))?;

            let field_count = &metadata.fields.len();

            csv_writer.write_record(&metadata.fields)?;

            for row in csv_reader.into_byte_records() {
                let mut this_row = row?;
                while &this_row.len() != field_count {
                    this_row.push_field(b"")
                }
                csv_writer.write_byte_record(&this_row)?;
            }
        }

        remove_dir_all(&tmp_path)?;

        let metadata_file = File::create(self.output_path.join("table_metadata.json"))?;
        serde_json::to_writer_pretty(metadata_file, &self.table_metadata)?;
        return Ok(())
    }

}

//fn value_convert(value: Value, mut output_fields: &IndexMap<String, IndexMap<String, String>>) -> String {
fn value_convert(value: Value) -> String {
    match value {
        Value::String(val) => {
            format!("{}", val)
        }
        Value::Null => {
            format!("")
        }
        Value::Number(num) => {
            format!("{}", num)
        }
        Value::Bool(bool) => {
            format!("{}", bool)
        }
        Value::Array(arr) => {
            format!("{}", Value::Array(arr))
        }
        Value::Object(obj) => {
            format!("{}", Value::Object(obj))
        }
    }
}

