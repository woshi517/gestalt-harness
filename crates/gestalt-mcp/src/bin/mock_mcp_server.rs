use serde_json::json;
use std::io::{self, BufRead, Write};

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }

        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let id = req.get("id").cloned();

        let response = match method {
            "initialize" => {
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {
                            "tools": {}
                        },
                        "serverInfo": {
                            "name": "mock-mcp-server",
                            "version": "1.0.0"
                        }
                    }
                })
            }
            "tools/list" => {
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": [
                            {
                                "name": "mock_tool",
                                "description": "A mock tool for testing",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "input": {
                                            "type": "string"
                                        }
                                    },
                                    "required": ["input"]
                                }
                            }
                        ]
                    }
                })
            }
            "tools/call" => {
                let args = req.get("params").and_then(|p| p.get("arguments"));
                let trigger_notif = args
                    .and_then(|a| a.get("input").and_then(|i| i.as_str()))
                    .map_or(false, |s| s == "trigger_list_changed");

                if trigger_notif {
                    let notification = json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/tools/list_changed",
                        "params": {}
                    });
                    if let Ok(notif_str) = serde_json::to_string(&notification) {
                        let _ = writeln!(stdout, "{}", notif_str);
                        let _ = stdout.flush();
                    }
                }
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [
                            {
                                "type": "text",
                                "text": "Mock tool response"
                            }
                        ],
                        "isError": false
                    }
                })
            }
            _ => {
                if id.is_some() {
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32601,
                            "message": "Method not found"
                        }
                    })
                } else {
                    continue;
                }
            }
        };

        if let Ok(resp_str) = serde_json::to_string(&response) {
            let _ = writeln!(stdout, "{}", resp_str);
            let _ = stdout.flush();
        }
    }
}
