// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
//
// SPDX-License-Identifier: GPL-3.0-or-later

mod cli;

fn main() -> std::io::Result<()> {
    use clap::Parser;
    let matches = cli::Cli::parse();

    colog::default_builder()
        .filter_level(match matches.verbose {
            0 => log::LevelFilter::Warn,
            1 => log::LevelFilter::Info,
            2 => log::LevelFilter::Debug,
            _ => log::LevelFilter::Trace,
        })
        .init();

    log::trace!("CLI options: {:?}", matches);

    for file in matches.input {
        log::trace!("Processing file: {}", file.display());

        let output: &mut dyn std::io::Write = match matches.inplace {
            false => {
                log::trace!("Writing to stdout");
                &mut std::io::stdout()
            }
            true => {
                let swap = file.with_extension("djotfmt.swp");

                log::trace!("Writing to swap file {}", swap.display());

                &mut std::fs::File::create_new(swap).expect("Swapping file already exists")
            }
        };

        log::trace!("Start render file");

        let input = std::fs::read_to_string(file.clone())?;

        let config = djotfmt::fmt::FmtConfig {
            max_cols: matches.columns,
        };

        let result = djotfmt::fmt::format(&input, &config);
        output.write_all(result.as_bytes())?;

        log::trace!("File rendered");

        if !matches.inplace {
            continue;
        }

        let swap = file.with_extension("djotfmt.swp");
        assert!(swap.exists(), "Swap file does not exist");

        log::trace!(
            "Renaming swap file {} to {}",
            swap.display(),
            file.display()
        );

        std::fs::rename(file.with_extension("djotfmt.swp"), file)?;
    }

    Ok(())
}
