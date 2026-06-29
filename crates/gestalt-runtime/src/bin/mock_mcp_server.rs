use serde_json::json;
use std::io::{self, BufRead, Write};

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }

        let request: serde_json::Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let method = request
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let id = request.get("id").cloned();

        let response = match method {
            "initialize" => json!({
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
            }),
            "tools/list" => json!({
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
            }),
            "tools/call" => {
                let trigger_notification = request
                    .get("params")
                    .and_then(|params| params.get("arguments"))
                    .and_then(|arguments| arguments.get("input"))
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|input| input == "trigger_list_changed");

                if trigger_notification {
                    let notification = json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/tools/list_changed",
                        "params": {}
                    });
                    if let Ok(notification_str) = serde_json::to_string(&notification) {
                        let _ = writeln!(stdout, "{notification_str}");
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
            _ if id.is_some() => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": "Method not found"
                }
            }),
            _ => continue,
        };

        if let Ok(response_str) = serde_json::to_string(&response) {
            let _ = writeln!(stdout, "{response_str}");
            let _ = stdout.flush();
        }
    }
}
