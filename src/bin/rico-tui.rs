//! Rico in the terminal.

fn main() -> anyhow::Result<()> {
    let opts = match rico::parse_args() {
        Ok(o) => o,
        Err(code) => std::process::exit(code),
    };
    let _log = rico::init();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(rico::tui::run(opts.demo))
}
