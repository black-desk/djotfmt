// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
// SPDX-License-Identifier: GPL-3.0-or-later

#[path = "../lsp.rs"]
mod lsp;

#[derive(clap::Parser)]
#[command(version, about = "Djot language server over stdio")]
struct Args {}

#[tokio::main]
async fn main() {
    use clap::Parser;
    Args::parse();
    let (service, socket) = tower_lsp_server::LspService::new(lsp::Backend::new);
    tower_lsp_server::Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
        .serve(service)
        .await;
}
