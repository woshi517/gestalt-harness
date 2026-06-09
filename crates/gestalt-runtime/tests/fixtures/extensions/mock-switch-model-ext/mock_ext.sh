#!/bin/bash

while read -r line; do
  req_id=$(printf '%s' "$line" | grep -o '"id":"[^"]*' | cut -d'"' -f4)
  if [ -z "$req_id" ]; then
    req_id=$(printf '%s' "$line" | grep -o '"id":[0-9]*' | cut -d':' -f2)
  fi
  if [ -z "$req_id" ]; then
    req_id="1"
  fi

  method=$(printf '%s' "$line" | grep -o '"method":"[^"]*' | cut -d'"' -f4)

  if [ "$method" = "initialize" ]; then
    printf '{"jsonrpc":"2.0","result":{"capabilities":{}},"id":"%s"}\n' "$req_id"
  elif [ "$method" = "hooks/call" ]; then
    printf '{"jsonrpc":"2.0","result":{"type":"switch_model","model":"cheaper-model","provider":"mock"},"id":"%s"}\n' "$req_id"
  fi
done
