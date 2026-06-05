#!/bin/bash
# Mock JSON-RPC stdio extension
while read -r line; do
  # Extract JSON-RPC ID (robust to both string/number IDs)
  req_id=$(echo "$line" | grep -o '"id":"[^"]*' | cut -d'"' -f4)
  if [ -z "$req_id" ]; then
    req_id=$(echo "$line" | grep -o '"id":[0-9]*' | cut -d':' -f2)
  fi

  method=$(echo "$line" | grep -o '"method":"[^"]*' | cut -d'"' -f4)

  if [ "$method" = "initialize" ]; then
    echo "{\"jsonrpc\":\"2.0\",\"result\":{\"capabilities\":{}},\"id\":\"$req_id\"}"
  elif [ "$method" = "tools/call" ]; then
    val_secret=${TEST_SECRET:-unset}
    val_path=${PATH:-unset}
    echo "{\"jsonrpc\":\"2.0\",\"result\":{\"content\":\"TEST_SECRET=$val_secret PATH=$val_path\"},\"id\":\"$req_id\"}"
  elif [ "$method" = "context/inject" ]; then
    echo "{\"jsonrpc\":\"2.0\",\"result\":{\"content\":\"injected context\"},\"id\":\"$req_id\"}"
  elif [ "$method" = "hooks/call" ]; then
    echo "{\"jsonrpc\":\"2.0\",\"result\":{\"type\":\"block\",\"reason\":\"blocked by mock extension hook\"},\"id\":\"$req_id\"}"
  fi
done
