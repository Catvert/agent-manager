use std::io::Cursor;

use anyhow::{Result, anyhow};
use skim::prelude::*;

pub fn skim_select(items: &[String], prompt: &str) -> Result<Option<usize>> {
    if items.is_empty() {
        return Ok(None);
    }

    let options = SkimOptionsBuilder::default()
        .multi(false)
        .height(Some("30%"))
        .prompt(Some(prompt))
        .build()
        .map_err(|err| anyhow!("Invalid skim configuration: {}", err))?;

    let display = items
        .iter()
        .map(|item| item.replace('\n', " "))
        .collect::<Vec<_>>();
    let input = display.join("\n");

    let reader = Cursor::new(input);
    let item_reader = SkimItemReader::default().of_bufread(reader);
    let output = Skim::run_with(&options, Some(item_reader));
    if let Some(out) = output {
        if out.is_abort {
            return Ok(None);
        }
        if let Some(item) = out.selected_items.first() {
            let value = item.output().to_string();
            if let Some(idx) = display.iter().position(|candidate| candidate == &value) {
                return Ok(Some(idx));
            }
        }
    }

    Ok(None)
}
