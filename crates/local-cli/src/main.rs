use anyhow::Result;
use clap::Parser;
use local_core::{Config, Node};
use std::{
    io::{BufRead, Write},
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
};

/// LoCAL LAN peer. Type help for commands, or use --json for newline-delimited JSON IPC.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    data_dir: Option<PathBuf>,
    #[arg(long)]
    receive_dir: Option<PathBuf>,
    #[arg(long, default_value_t = 53319)]
    port: u16,
    #[arg(long)]
    no_discovery: bool,
    #[arg(long)]
    json: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let mut config = Config::default();
    if let Some(v) = args.data_dir {
        config.data_dir = v;
    }
    if let Some(v) = args.receive_dir {
        config.receive_dir = v;
    }
    if let Some(v) = args.name {
        config.name = v;
    }
    config.bind = SocketAddr::from((Ipv4Addr::UNSPECIFIED, args.port));
    config.discovery = !args.no_discovery;
    let node = Node::start(config).await?;
    if !args.json {
        println!("LoCAL {} · QUIC / TLS 1.3", env!("CARGO_PKG_VERSION"));
        println!(
            "Device: {}\nListening: {}\nType help for commands.",
            node.id,
            node.address()?
        );
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            match line {
                Ok(line) => {
                    if tx.blocking_send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    loop {
        if !args.json {
            print!("local> ");
            std::io::stdout().flush()?;
        }
        let line = tokio::select! { line=rx.recv()=>match line {Some(line)=>line,None=>break},_=tokio::signal::ctrl_c()=>break };
        let line = line.trim();
        if line == "quit" || line == "exit" {
            break;
        }
        if line == "help" && !args.json {
            println!("status | connect IP:PORT | pair PEER_ID CODE | text PEER_ID MESSAGE\nfile PEER_ID PATH | accept TRANSFER_ID | reject TRANSFER_ID | cancel TRANSFER_ID\nforget PEER_ID | name DEVICE_NAME | quit\nJSON commands are also accepted. Use status to see IDs, pairing codes, and offers.");
            continue;
        }
        let command = if args.json || line.starts_with('{') {
            serde_json::from_str(line).map_err(anyhow::Error::from)
        } else {
            parse(line)
        };
        let result = match command {
            Ok(value) => node.command(value).await,
            Err(e) => Err(e),
        };
        let result = match result {
            Ok(data) => serde_json::json!({"ok":true,"data":data}),
            Err(e) => serde_json::json!({"ok":false,"error":e.to_string()}),
        };
        if args.json {
            println!("{result}");
        } else {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }
    node.stop();
    Ok(())
}

fn parse(line: &str) -> Result<serde_json::Value> {
    use serde_json::json;
    let mut parts = line.splitn(3, ' ');
    let command = parts.next().unwrap_or("");
    let a = parts.next().unwrap_or("");
    let b = parts.next().unwrap_or("");
    Ok(match command {
        "status" => json!({"op":"snapshot"}),
        "connect" => json!({"op":"connect","address":a}),
        "pair" => json!({"op":"confirm","peer_id":a,"code":b}),
        "text" => json!({"op":"send_text","peer_id":a,"text":b}),
        "file" => json!({"op":"send_file","peer_id":a,"path":b.trim_matches('"')}),
        "accept" | "reject" => json!({"op":"accept_file","id":a,"accept":command=="accept"}),
        "cancel" => json!({"op":"cancel_transfer","id":a}),
        "forget" => json!({"op":"forget","peer_id":a}),
        "name" => json!({"op":"set_name","name":line.strip_prefix("name ").unwrap_or("")}),
        _ => anyhow::bail!("Unknown command; type help"),
    })
}
