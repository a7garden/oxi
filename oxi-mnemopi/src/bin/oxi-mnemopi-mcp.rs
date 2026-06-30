//! `oxi-mnemopi-mcp` — MCP server binary exposing the Mnemopi engine
//! over stdio JSON-RPC 2.0.
//!
//! Ported from omp `packages/mnemopi/src/mcp-server.ts`.
//!
//! # Usage
//!
//! ```sh
//! oxi-mnemopi-mcp --db-path ~/.oxi/memory/project.db [--session-id default]
//! ```
//!
//! Embeddings default to off. Set `OXI_MNEMOPI_EMBEDDING_MODEL` to enable
//! remote embeddings via the OpenAI-compatible `/v1/embeddings` endpoint
//! configured by `OXI_MNEMOPI_EMBEDDING_URL` + `OXI_MNEMOPI_EMBEDDING_KEY`.

use std::path::PathBuf;

use oxi_mnemopi::mcp::{McpServer, McpServerOptions};

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let mut db_path: Option<PathBuf> = None;
    let mut session_id = String::from("default");

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--db-path" | "-d" => {
                db_path = args.next().map(PathBuf::from);
            }
            "--session-id" | "-s" => {
                if let Some(s) = args.next() {
                    session_id = s;
                }
            }
            "--help" | "-h" => {
                eprintln!("oxi-mnemopi-mcp — MCP server for the Mnemopi memory engine");
                eprintln!();
                eprintln!("USAGE:");
                eprintln!("    oxi-mnemopi-mcp --db-path <PATH> [--session-id <ID>]");
                eprintln!();
                eprintln!("OPTIONS:");
                eprintln!("    -d, --db-path <PATH>      Path to the SQLite database (required)");
                eprintln!("    -s, --session-id <ID>     Logical session ID [default: default]");
                eprintln!("    -h, --help                Print this help message");
                eprintln!();
                eprintln!("ENVIRONMENT:");
                eprintln!(
                    "    OXI_MNEMOPI_EMBEDDING_MODEL   Embedding model name (enables remote embeddings)"
                );
                eprintln!(
                    "    OXI_MNEMOPI_EMBEDDING_URL     OpenAI-compatible /v1/embeddings endpoint"
                );
                eprintln!("    OXI_MNEMOPI_EMBEDDING_KEY     API key for the embedding endpoint");
                return Ok(());
            }
            _ => {
                eprintln!("unknown argument: {arg}. Use --help for usage.");
                std::process::exit(2);
            }
        }
    }

    let db_path = db_path.unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".oxi")
            .join("memory")
            .join("mcp.db")
    });

    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let embedding_model = std::env::var("OXI_MNEMOPI_EMBEDDING_MODEL").unwrap_or_default();
    let embedding_provider: Option<std::sync::Arc<dyn oxi_mnemopi::EmbeddingProvider>> =
        if embedding_model.is_empty() {
            None
        } else {
            #[cfg(feature = "remote-embeddings")]
            {
                let url = std::env::var("OXI_MNEMOPI_EMBEDDING_URL")
                    .unwrap_or_else(|_| "https://api.openai.com/v1/embeddings".to_string());
                let key = std::env::var("OXI_MNEMOPI_EMBEDDING_KEY").unwrap_or_default();
                match oxi_mnemopi::RemoteEmbeddingProvider::new(&url, &key, &embedding_model) {
                    Ok(p) => {
                        eprintln!(
                            "oxi-mnemopi-mcp: remote embeddings enabled (model={embedding_model})"
                        );
                        Some(std::sync::Arc::new(p))
                    }
                    Err(e) => {
                        eprintln!("oxi-mnemopi-mcp: failed to init embedding provider: {e}");
                        None
                    }
                }
            }
            #[cfg(not(feature = "remote-embeddings"))]
            {
                eprintln!(
                    "oxi-mnemopi-mcp: OXI_MNEMOPI_EMBEDDING_MODEL set but remote-embeddings feature is disabled"
                );
                None
            }
        };

    let server = match McpServer::open(McpServerOptions {
        db_path,
        session_id,
        embedding_provider,
        embedding_model,
    }) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("oxi-mnemopi-mcp: failed to open engine: {e}");
            std::process::exit(1);
        }
    };

    eprintln!("oxi-mnemopi-mcp: listening on stdio (JSON-RPC 2.0)");

    // Wrap stdin in BufReader so it satisfies `AsyncBufRead`.
    let stdin = tokio::io::BufReader::new(tokio::io::stdin());
    let stdout = tokio::io::stdout();
    server.run_stdio(stdin, stdout).await
}
