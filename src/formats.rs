use std::iter::{repeat};
use std::path::Path;
use std::fs::File;
use clap::ArgEnum;
use serde::Serialize;
use serde_json::Value;
use serde_json::de::from_reader;

#[derive(Debug, Clone, ArgEnum, Serialize)]
pub enum InputFormat {
    Dreamcoder,
    ProgramsList,
    SplitProgramsList,
}

#[derive(Debug, Clone)]
pub struct Input {
    pub train_programs: Vec<String>, // Program strings. 
    pub test_programs: Option<Vec<String>>, // Program strings. 
    pub tasks: Option<Vec<String>>, // Task names for each corresponding string.
    pub prev_dc_inv_to_inv_strs: Option<Vec<(String, String)>>, // Vec of [#Dreamcoder invention, fn_i] tuples for any existing inventions in the DSL.
}

impl InputFormat {
    pub fn load_programs_and_tasks(&self, path: &Path) -> Result<Input, String> {
        match *self {
            InputFormat::Dreamcoder => {
                // read dreamcoder format
                let json: Value = from_reader(File::open(path).expect("file not found")).expect("json deserializing error");
                // there should be a "frontiers" field at the toplevel
                let frontiers = json["frontiers"].as_array().unwrap_or_else(||panic!("json parse error, are you sure you wanted format {:?}?", self));
                // grab any existing inventions from the DSL
                let mut dc_invs: Vec<String> = json["DSL"]["productions"].as_array().unwrap().iter().map(|prod|prod["expression"].as_str().unwrap().to_string())
                    .filter(|s| s.starts_with('#'))
                    .collect();
                dc_invs.sort_by_key(|s| s.len()); // increasing length so inventions that build on earlier ones come later
                let inv_dc_strs: Vec<(String, String)> = dc_invs
                    .into_iter()
                    .enumerate()
                    .map(|(i, dc_str)| (format!("prev_dc_inv_{}", i), dc_str)) // TODO: determine if we need to replace these in the future.
                    .collect();
                let mut programs: Vec<String> = Vec::default();
                let mut tasks: Vec<String> = Vec::default();
                for (i,frontier) in frontiers.iter().enumerate() {
                    let programs_in_frontier: Vec<String> = frontier["programs"].as_array().unwrap().iter().map(|p|p["program"].as_str().unwrap().to_string())
                        .map(|p| inv_dc_strs.iter().rev().fold(p, |p, s| p.replace(&s.1, &s.0))) // replace #(lambda ...) with fn_2 etc. Start with highest numbered fn to avoid mangling bodies of other fns.
                        .collect();
                    assert!(!programs_in_frontier.iter().any(|p| p.contains('#')));
                    let task: String = match frontier["task"].as_str(){
                        Some(name) => name.to_string(),
                        None => i.to_string()
                    };
                    let task_repeated: Vec<String> = repeat(task).take(programs_in_frontier.len()).collect();
                    programs.extend(programs_in_frontier);
                    tasks.extend(task_repeated);
                }
                let input = Input {
                    train_programs: programs,
                    test_programs: None,
                    tasks: Some(tasks),
                    prev_dc_inv_to_inv_strs: Some(inv_dc_strs),
                };
                Ok(input)
            }
            InputFormat::ProgramsList => {
                let programs: Vec<String> = from_reader(File::open(path).map_err(|e| format!("file not found, error code {:?}", e))?).map_err(|e| format!("json parser error, are you sure you wanted format {:?}? Error code was {:?}", self, e))?;
                let input = Input {
                    train_programs: programs,
                    test_programs: None,
                    tasks: None,
                    prev_dc_inv_to_inv_strs: None,
                };
                Ok(input)
            }
            InputFormat::SplitProgramsList => {
                let programs: Vec<Vec<String>> = from_reader(File::open(path).map_err(|e| format!("file not found, error code {:?}", e))?).map_err(|e| format!("json parser error, are you sure you wanted format {:?}? Error code was {:?}", self, e))?;
                assert_eq!(programs.len(), 2);
                let train_programs = programs.get(0).unwrap().clone();
                let test_programs = programs.get(1).unwrap().clone();
                let input = Input {
                    train_programs,
                    test_programs: Some(test_programs),
                    tasks: None,
                    prev_dc_inv_to_inv_strs: None,
                };
                Ok(input)
            }
        }
    }
}
