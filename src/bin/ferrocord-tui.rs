//! ferrocord da terminale.

fn main() -> anyhow::Result<()> {
    let opts = match ferrocord::parse_args() {
        Ok(o) => o,
        Err(code) => std::process::exit(code),
    };
    let _log = ferrocord::init();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(ferrocord::tui::run(opts.demo))
}
