#!/bin/bash

# Check if the input file is provided
if [ -z "$1" ]; then
  echo "Usage: $0 <file>"
  exit 1
fi

input_file="$1"

# Read the input file line by line
while IFS= read -r npub; do
  # Run the command for each npub and output the result
  result=$(nostrkeytool --npub2pubkey "$npub")
  echo "$result"
done < "$input_file"

