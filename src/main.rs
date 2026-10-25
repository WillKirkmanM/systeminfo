use clap::{Parser, Subcommand};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const COLUMNS: [&str; 5] = [
    "Host Name",
    "OS Version",
    "System Manufacturer",
    "System Model",
    "Total Physical Memory",
];

#[derive(Parser)]
#[command(
    version,
    about = "Inspect and compare saved Windows system inventories"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Inspect a header-bearing systeminfo CSV inventory in host-name order")]
    Inspect {
        #[arg(help = "Saved CSV inventory")]
        input: PathBuf,
    },
    #[command(about = "Compare two saved inventories by host name, ignoring case")]
    Compare {
        #[arg(help = "Earlier CSV inventory")]
        before: PathBuf,
        #[arg(help = "Later CSV inventory")]
        after: PathBuf,
    },
}

#[derive(Debug, PartialEq)]
struct Host {
    name: String,
    fields: [String; 4],
}

type Inventory = BTreeMap<String, Host>;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Start,
    Plain,
    Quoted,
    Closed,
}

fn parse_csv(input: &str) -> Result<Vec<Vec<String>>, String> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut state = State::Start;
    let mut started = false;
    let mut line = 1;
    let mut chars = input
        .strip_prefix('\u{feff}')
        .unwrap_or(input)
        .chars()
        .peekable();

    while let Some(ch) = chars.next() {
        if state == State::Quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    state = State::Closed;
                }
            } else {
                field.push(ch);
                if ch == '\n' {
                    line += 1;
                }
            }
            continue;
        }

        match ch {
            ',' => {
                record.push(std::mem::take(&mut field));
                state = State::Start;
                started = true;
            }
            '\n' | '\r' => {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                if started {
                    record.push(std::mem::take(&mut field));
                    records.push(std::mem::take(&mut record));
                }
                state = State::Start;
                started = false;
                line += 1;
            }
            '"' if state == State::Start => {
                state = State::Quoted;
                started = true;
            }
            '"' => {
                return Err(format!(
                    "line {line}: unexpected quote in an unquoted field"
                ));
            }
            _ if state == State::Closed => {
                return Err(format!(
                    "line {line}: unexpected character after a closing quote"
                ));
            }
            _ => {
                field.push(ch);
                state = State::Plain;
                started = true;
            }
        }
    }

    if state == State::Quoted {
        return Err(format!("line {line}: unterminated quoted field"));
    }
    if started {
        record.push(field);
        records.push(record);
    }
    Ok(records)
}

fn parse_inventory(input: &str) -> Result<Inventory, String> {
    let records = parse_csv(input)?;
    let header = records.first().ok_or("missing CSV header")?;
    let mut seen = HashSet::new();
    for column in header {
        if !seen.insert(column) {
            return Err(format!("duplicate column {column:?}"));
        }
    }
    let mut indices = [0; 5];
    for (index, name) in COLUMNS.iter().enumerate() {
        indices[index] = header
            .iter()
            .position(|column| column == name)
            .ok_or_else(|| format!("missing required column {name:?}"))?;
    }

    let mut inventory = Inventory::new();
    for (index, row) in records.iter().enumerate().skip(1) {
        let record_number = index + 1;
        if row.len() != header.len() {
            return Err(format!(
                "record {record_number}: expected {} fields, found {}",
                header.len(),
                row.len()
            ));
        }
        let name = row[indices[0]].trim();
        if name.is_empty() {
            return Err(format!("record {record_number}: empty host name"));
        }
        let key = name.to_lowercase();
        if inventory.contains_key(&key) {
            return Err(format!(
                "record {record_number}: duplicate host name {name:?}"
            ));
        }
        inventory.insert(
            key,
            Host {
                name: name.to_owned(),
                fields: std::array::from_fn(|i| row[indices[i + 1]].clone()),
            },
        );
    }
    Ok(inventory)
}

fn read_inventory(path: &Path) -> Result<Inventory, String> {
    let input = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    parse_inventory(&input).map_err(|error| format!("{}: {error}", path.display()))
}

fn inspect(inventory: &Inventory) -> String {
    if inventory.is_empty() {
        return "No hosts.\n".to_owned();
    }
    let mut output = String::new();
    for host in inventory.values() {
        writeln!(output, "Host: {:?}", host.name).unwrap();
        for (label, value) in COLUMNS[1..].iter().zip(&host.fields) {
            writeln!(output, "  {label}: {value:?}").unwrap();
        }
    }
    output
}

fn compare(before: &Inventory, after: &Inventory) -> String {
    let keys: std::collections::BTreeSet<_> = before.keys().chain(after.keys()).collect();
    let mut output = String::new();
    for key in keys {
        match (before.get(key), after.get(key)) {
            (None, Some(host)) => writeln!(output, "Added host: {:?}", host.name).unwrap(),
            (Some(host), None) => writeln!(output, "Removed host: {:?}", host.name).unwrap(),
            (Some(old), Some(new)) if old.fields != new.fields => {
                writeln!(output, "Changed host: {:?}", new.name).unwrap();
                for ((label, old), new) in COLUMNS[1..].iter().zip(&old.fields).zip(&new.fields) {
                    if old != new {
                        writeln!(output, "  {label}: {old:?} -> {new:?}").unwrap();
                    }
                }
            }
            _ => {}
        }
    }
    if output.is_empty() {
        output.push_str("No differences.\n");
    }
    output
}

fn run(cli: Cli) -> Result<String, String> {
    match cli.command {
        Command::Inspect { input } => Ok(inspect(&read_inventory(&input)?)),
        Command::Compare { before, after } => {
            Ok(compare(&read_inventory(&before)?, &read_inventory(&after)?))
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str =
        "Host Name,OS Version,System Manufacturer,System Model,Total Physical Memory\n";

    #[test]
    fn parses_quoted_commas_quotes_and_multiline_fields() {
        let input = format!(
            "\u{feff}{HEADER}\"DESKTOP-A\",10.0.26100,\"Example \"\"Systems\"\"\",\"Workstation,\r\nModel A\",\"16,384 MB\"\r\n"
        );
        let inventory = parse_inventory(&input).unwrap();
        assert_eq!(inventory["desktop-a"].fields[1], "Example \"Systems\"");
        assert_eq!(inventory["desktop-a"].fields[2], "Workstation,\r\nModel A");
        assert_eq!(inventory["desktop-a"].fields[3], "16,384 MB");
    }

    #[test]
    fn rejects_missing_columns_duplicate_hosts_and_bad_rows() {
        assert!(parse_inventory("Host Name,OS Version\nA,1\n").is_err());
        assert!(parse_inventory(&format!("{HEADER}A,1,M,X,2\na,1,M,X,2\n")).is_err());
        assert!(parse_inventory(&format!("{HEADER}A,1\n")).is_err());
        assert!(parse_inventory(&format!("{HEADER},1,M,X,2\n")).is_err());
    }

    #[test]
    fn rejects_malformed_csv() {
        for input in ["\"unterminated", "a\"b,c", "\"a\"b,c"] {
            assert!(parse_csv(input).is_err(), "accepted {input:?}");
        }
    }

    #[test]
    fn compares_hosts_ignoring_case_and_reports_each_change() {
        let before = parse_inventory(&format!("{HEADER}A,1,M,X,2\nB,1,M,X,2\n")).unwrap();
        let after = parse_inventory(&format!("{HEADER}a,3,N,Y,4\nC,1,M,X,2\n")).unwrap();
        assert_eq!(
            compare(&before, &after),
            "Changed host: \"a\"\n  OS Version: \"1\" -> \"3\"\n  System Manufacturer: \"M\" -> \"N\"\n  System Model: \"X\" -> \"Y\"\n  Total Physical Memory: \"2\" -> \"4\"\nRemoved host: \"B\"\nAdded host: \"C\"\n"
        );
        let same = parse_inventory(&format!("{HEADER}a,1,M,X,2\nB,1,M,X,2\n")).unwrap();
        assert_eq!(compare(&before, &same), "No differences.\n");
    }

    #[test]
    fn accepts_reordered_extra_columns_and_sorts_hosts() {
        let inventory = parse_inventory(
            "Extra,System Model,Host Name,Total Physical Memory,OS Version,System Manufacturer\nz,X,Z,2,1,M\na,Y,a,4,3,N\n",
        )
        .unwrap();
        assert_eq!(inventory["a"].fields, ["3", "N", "Y", "4"]);
        assert!(inspect(&inventory).starts_with("Host: \"a\"\n"));
        assert!(parse_inventory(HEADER).unwrap().is_empty());
    }
}
