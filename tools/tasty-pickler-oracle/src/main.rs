use std::{env, fs, process};

use tasty_pickler_oracle::{MAX_FILE_BYTES, Result, inspect, reduced_answer};

fn run() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    let [command, input, rest @ ..] = args.as_slice() else {
        return Err("usage: tasty-pickler-oracle inspect FILE [--raw-addresses] | reduce-answer FILE OUTPUT".into());
    };
    if fs::metadata(input)?.len() > MAX_FILE_BYTES as u64 {
        return Err("oracle input exceeds 1 MiB".into());
    }
    let bytes = fs::read(input)?;
    match (command.as_str(), rest) {
        ("inspect", []) => println!(
            "{}",
            serde_json::to_string_pretty(&inspect(&bytes, false)?)?
        ),
        ("inspect", [flag]) if flag == "--raw-addresses" => {
            println!("{}", serde_json::to_string_pretty(&inspect(&bytes, true)?)?);
        }
        ("reduce-answer", [output]) => fs::write(output, reduced_answer(&bytes)?)?,
        _ => return Err("invalid oracle command/arguments".into()),
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("tasty-pickler-oracle: {error}");
        process::exit(1);
    }
}
