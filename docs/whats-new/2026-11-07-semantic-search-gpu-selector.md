### AI semantic search hardware device selection and status
- Natural-language semantic search background indexing and phrase embedding can now be pinned to a specific GPU adapter or forced to CPU in Settings ▸ Performance and Settings ▸ AI Models.
- Hardware probe and status truthfully report CPU execution and fallback reasons (`whyCpu`) on platforms where neural model weights run on the multithreaded CPU engine, avoiding false GPU reports.
