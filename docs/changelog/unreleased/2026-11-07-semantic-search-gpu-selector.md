## Performance

- **AI semantic search hardware selector:** Settings ▸ Performance and Settings ▸ AI Models now include an explicit device selector for AI natural-language semantic search embeddings, choosing between Automatic, detected physical GPUs, and CPU. Status reporting truthfully reports active hardware with fallback reasons (`whyCpu`) and properly binds device indices on Metal/macOS. PR #868.
