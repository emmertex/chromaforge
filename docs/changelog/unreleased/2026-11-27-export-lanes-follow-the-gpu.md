## Performance
- Automatic export lanes follow the GPU's measured share of the work: one fewer per stretch of photos while it is at least 90 % busy, one more while it is under 70 %, between two and the automatic count (#1377). Lanes set by hand or by Test my machine are kept as they are.
