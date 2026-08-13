use std::io::Write;

use buzz_x0x_mcp::{handle_request, parse_error_response, RuntimeConfig, X0xTools};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};

const MAX_MCP_FRAME_BYTES: usize = 256 * 1024;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = RuntimeConfig::from_env()?;
    let mut tools = X0xTools::new(config)?;
    tools.resolve_stable_group_id().await?;
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut frame = Vec::new();

    loop {
        frame.clear();
        let bytes_read = reader.read_until(b'\n', &mut frame).await?;
        if bytes_read == 0 {
            break;
        }
        if frame.len() > MAX_MCP_FRAME_BYTES {
            write_message(&parse_error_response())?;
            continue;
        }
        let request: Value = match serde_json::from_slice(&frame) {
            Ok(value) => value,
            Err(_) => {
                write_message(&parse_error_response())?;
                continue;
            }
        };
        if let Some(response) = handle_request(&tools, &request).await {
            write_message(&response)?;
        }
    }
    Ok(())
}

fn write_message(message: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = serde_json::to_vec(message)?;
    bytes.push(b'\n');
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&bytes)?;
    stdout.flush()?;
    Ok(())
}
