### MCP import refuses a malformed paths list

- An agent's `import` request with a non-string entry in `paths` is now refused whole, naming the entry (`paths[1]`), instead of importing (and in Move mode removing) the valid ones.
