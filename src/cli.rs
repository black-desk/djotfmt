#[derive(clap::Parser, Debug)]
#[command(
    version,
    about = "A tool to format Djot markup documentation.",
    long_about= include_str!("../docs/long_about.txt").trim(),
    after_help= include_str!("../docs/after_help.txt").trim(),
)]
pub struct Cli {
    #[arg(short, long, action = clap::ArgAction::Count, help = "Set verbosity level")]
    pub verbose: u8,

    #[arg(default_value = "/dev/stdin", help = "Input file(s)")]
    pub input: Vec<std::path::PathBuf>,

    #[clap(short, help = "Inplace edit <file>s")]
    pub inplace: bool,
}
