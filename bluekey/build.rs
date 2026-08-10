use std::env;
use std::fmt::Display;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::io::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LiteralTypes {
    Hex,
    Decimal,
    Octal,
    Binary
}
impl LiteralTypes {
    fn radix(&self) -> u32 {
        match self {
            Self::Binary => 2,
            Self::Decimal => 10,
            Self::Octal => 8,
            Self::Hex => 16
        }
    }
}
impl Display for LiteralTypes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", match self {
            Self::Binary => "binary",
            Self::Decimal => "decimal",
            Self::Octal => "octal",
            Self::Hex => "hexadecimal"
        })
    }
}


fn skip_whitespace(characters: &mut std::iter::Peekable<impl Iterator<Item = char>>) {
    while let Some(character) = characters.peek() && character.is_whitespace() {
        characters.next();
    }
}
fn parse_name(characters: &mut std::iter::Peekable<impl Iterator<Item = char>>) -> String {
    let mut name = String::new();

    while let Some(character) = characters.peek() {
        if character.is_alphanumeric() || *character == '_' {
            name.push(*character);
            characters.next();
        } else {
            break
        }
    };

    name
}

fn read_defines(file: std::fs::File, log: &mut std::fs::File) -> impl Iterator<Item=(String, u16)> {
   BufReader::new(file)
        .lines()
        .filter_map(|line| line.unwrap().strip_prefix("#define").map(|line| String::from(line))) 
        .filter_map(move |line| {
            let mut characters = line.chars().peekable();
            
            // Parse the #define's name
            skip_whitespace(&mut characters);
            let mut name = parse_name(&mut characters);

            // Ignore #define's that aren't keys
            writeln!(log, "Found name: {}", name).unwrap();
            match name.starts_with("KEY_") {
                true => name.drain(.."KEY_".len()),
                false => return None
            };

            // Look at the first character of the integer
            skip_whitespace(&mut characters);
            let first = characters.next().unwrap();
            if !first.is_numeric() {
                // Some keys are defined as other keys, effectively aliased. 
                // Only the first definition will be used for the name mapping, but this is not an error.
                match &*parse_name(&mut characters) {
                    "" => println!("cargo::warning=Invalid digit '{}' for {}", first, name),
                    alternate => writeln!(log, "Key {} has alternate name {}, ignored", name, alternate).unwrap()
                }
                return None
            }
            
            // Find the type of the integer
            let mut value = 0;
            let mode = match first {
                '0' => match characters.peek() {
                    Some('x') => {characters.next(); LiteralTypes::Hex},
                    Some('b') => {characters.next(); LiteralTypes::Binary}
                    Some(c) if c.is_numeric()  => LiteralTypes::Octal,
                    _ => LiteralTypes::Decimal
                },
                digit => match digit.to_digit(10) {
                    Some(digit) => {value += digit; LiteralTypes::Decimal},
                    _ => {
                        println!("cargo::warning=Invalid digit '{}' for {}, ignoring key", digit, name);
                        return None;
                    }
                }
            };

            // Any number that has a prefix should be followed by at least 1 next character.
            if mode != LiteralTypes::Decimal && characters.peek() == None {
                println!("cargo::warning=Invalid {} literal for key {}: Missing digits, ignoring keys", mode, name);
                return None;
            }

            // Parse the rest of the integer
            writeln!(log, "Integer of type: {:?}", mode).unwrap();
            while let Some(digit) = characters.peek() {
                if digit.is_whitespace() {
                    break
                } else if *digit == '\'' {
                    characters.next();
                    continue;
                }

                value = value*mode.radix() + digit.to_digit(mode.radix()).expect("Invalid integer literal");
                characters.next();
            }

            // Keycodes should always be u16
            Some((name, value.try_into().expect("Value out of range")))
        })
}


const DEFINITION_PATH: &'static str = "./src/input-event-codes.h";
const OUTPUT_PATH: &'static str = "evdev-code-names.rs";

fn main() {
    let output_directory = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let mut log = std::fs::File::create(output_directory.join("log.txt")).unwrap();

    // The evdev key list changes with relative frequency, so generate the name mapping from the keys manually.
    let file = std::fs::File::open(DEFINITION_PATH).unwrap();

    let mut result = std::fs::File::create(output_directory.join(OUTPUT_PATH)).unwrap();
    writeln!(result, "pub fn evdev_keycode_to_name(code: u16) -> Option<&'static str> {{\n  match code {{").unwrap();
    for (name, value) in read_defines(file, &mut log) {
        write!(result, "    {} => Some(\"{}\"),\n", value, name).unwrap();
    }
    writeln!(result, "    _ => None").unwrap();
    writeln!(result, "  }}").unwrap();
    writeln!(result, "}}").unwrap();

    println!("cargo::rerun-if-changed=src/input-event-codes.h");
}


