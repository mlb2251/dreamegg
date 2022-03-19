use std::iter::{repeat};
use std::path::Path;
use std::fs::File;
use clap::ArgEnum;
use serde::Serialize;
use serde_json::de::from_reader;

#[derive(Debug, Clone, ArgEnum, Serialize)]
pub enum InputFormat {
    Dreamcoder,
    ProgramsList
}

impl InputFormat {
    pub fn load_programs_and_tasks(&self, path: &Path) -> Result<(Vec<String>, Vec<String>), String> {
        match self {
            &InputFormat::Dreamcoder => {
                // read dreamcoder format
                let json: serde_json::Value = from_reader(File::open(path).expect("file not found")).expect("json deserializing error");
                let frontiers = json["frontiers"].as_array().unwrap_or_else(||panic!("json parse error, are you sure you wanted format {:?}?", self));
                let dsl: Vec<String> = json["DSL"]["productions"].as_array().unwrap().iter().map(|prod|prod["expression"].as_str().unwrap().to_string()).collect();
                let inv_dc_strs: Vec<(String,String)> = dsl.iter().rev() // rev() since the first dsl function is the newest
                    .filter(|s| s.starts_with("#")).cloned().enumerate()
                    .map(|(i,dc_str)| (format!("dc_fn_{}",i),dc_str)).collect();
                let mut programs: Vec<String> = Vec::default();
                let mut tasks: Vec<String> = Vec::default();
                for frontier in frontiers {
                    let programs_in_frontier: Vec<String> = frontier["programs"].as_array().unwrap().iter().map(|p|p["program"].as_str().unwrap().to_string())
                        .map(|p| inv_dc_strs.iter().rev().fold(p, |p, s| p.replace(&s.1, &s.0))) // replace #(lambda ...) with fn_2 etc. Start with highest numbered fn to avoid mangling bodies of other fns.
                        .map(|p| p.replace("(lambda ","(lam ")).collect();
                    assert!(!programs_in_frontier.iter().any(|p| p.contains("#")));
                    let task: String = frontier["task"].as_str().unwrap().to_string();
                    let task_repeated: Vec<String> = repeat(task).take(programs_in_frontier.len()).collect();
                    programs.extend(programs_in_frontier);
                    tasks.extend(task_repeated);
                }
                Ok((programs, tasks))
            }
            &InputFormat::ProgramsList => {
                let programs: Vec<String> = from_reader(File::open(path).map_err(|e| format!("file not found, error code {:?}", e))?).map_err(|e| format!("json parser error, are you sure you wanted format {:?}? Error code was {:?}", self, e))?;
                let mut tasks: Vec<String> = Vec::with_capacity(programs.len());
                let mut task_num: usize = 0;
                for _ in programs.iter() {
                    tasks.push(task_num.to_string());
                    task_num += 1;
                }
                Ok((programs, tasks))
            }
        }
    }
}