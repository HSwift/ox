use jargon_args::{Jargon, Key};
use std::io;
use std::io::BufRead;

/// Holds the version number of the crate
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Holds the help dialog
pub const HELP: &str = "\
Ox: A lightweight and flexible text editor

USAGE: ox [options] [files]

OPTIONS:
  --help, -h                 : Show this help message
  --version, -v              : Show the version number
  --config [path], -c [path] : Specify the configuration file
  --readonly, -r             : Prevent opened files from writing
  --filetype [ext], -f [ext] : Set the file type of files opened
  --stdin                    : Reads file from the stdin

EXAMPLES:
  ox
  ox test.txt
  ox test.txt test2.txt
  ox /home/user/docs/test.txt
  ox -c config.lua test.txt
  ox -r -c ~/.config/.oxrc -f lua my_file.lua
  tree | ox -r --stdin\
";

/// Read from the standard input
pub fn get_stdin() -> String {
    io::stdin().lock().lines().fold(String::new(), |acc, line| {
        acc + &line.unwrap_or_else(|_| String::new()) + "\n"
    })
}

/// Flags for command line interface
#[allow(clippy::struct_excessive_bools)]
pub struct CommandLineInterfaceFlags {
    pub help: bool,
    pub version: bool,
    pub read_only: bool,
    pub stdin: bool,
}

/// Struct to help with starting ox
pub struct CommandLineInterface {
    pub flags: CommandLineInterfaceFlags,
    pub file_type: Option<String>,
    pub config_path: String,
    pub to_open: Vec<String>,
}

impl CommandLineInterface {
    /// Create a new command line interface helper
    pub fn new() -> Self {
        // Start parsing
        let mut j = Jargon::from_env();

        // Define keys
        let filetype: Key = ["-f", "--filetype"].into();
        let config: Key = ["-c", "--config"].into();

        Self {
            flags: CommandLineInterfaceFlags {
                help: j.contains(["-h", "--help"]),
                version: j.contains(["-v", "--version"]),
                read_only: j.contains(["-r", "--readonly"]),
                stdin: j.contains("--stdin"),
            },
            file_type: j.option_arg::<String, Key>(filetype.clone()),
            config_path: j
                .option_arg::<String, Key>(config.clone())
                .unwrap_or_else(|| "~/.oxrc".to_string()),
            to_open: j.finish(),
        }
    }

    /// Handle options that won't need to start the editor
    pub fn basic_options(&self) {
        if self.flags.help {
            println!("{HELP}");
            std::process::exit(0);
        } else if self.flags.version {
            println!("{VERSION}");
            std::process::exit(0);
        }
    }
}
