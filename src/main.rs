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

        // Wrapper around `std::io::Write` that implements `std::fmt::Write`.
        struct Writer<W: std::io::Write> {
            inner: W,
        }
        impl<W: std::io::Write> Writer<W> {
            fn new(inner: W) -> Self {
                Writer { inner }
            }
        }
        impl<W: std::io::Write> std::fmt::Write for Writer<W> {
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                self.inner
                    .write_all(s.as_bytes())
                    .map_err(|_| std::fmt::Error)
            }
        }

        let output: &mut dyn std::fmt::Write = match matches.inplace {
            false => {
                log::trace!("Writing to stdout");

                &mut Writer::new(std::io::stdout())
            }
            true => {
                let swap = file.with_extension("djotfmt.swp");

                log::trace!("Writing to swap file {}", swap.display());

                &mut Writer::new(
                    std::fs::File::create_new(swap).expect("Swapping file already exists"),
                )
            }
        };

        log::trace!("Start render file");

        let input = std::fs::read_to_string(file.clone())?;
        let input = input.as_str();

        let config = djotfmt::WriterConfig {
            max_cols: matches.columns,
        };

        djotfmt::Renderer::new(input)
            .push_offset(
                jotdown::Parser::new(input).into_offset_iter(),
                output,
                &config,
            )
            .unwrap();

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
