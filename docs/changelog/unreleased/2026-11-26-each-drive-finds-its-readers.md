## Import

- **Each drive finds its own number of readers:** the I/O pool learns a limit per drive from its throughput and the whole process's (one drive tries a change at a time, judged against fresh measurements before and after), starting from the previous thread count and capped at 32 reads at once across all drives; a drive that several cards are copied into keeps at least two copies at once; a drive with no files for 10 minutes is forgotten. The import's read-ahead per drive grows with its limit.
