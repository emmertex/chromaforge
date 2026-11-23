### Faster raw repair passes
- **Remove Hot Pixels** and **Fix Hot and Dead Pixels** find the same pixels as before, about 3.5 times faster (issue #1377). On one thread a 45 MP Nikon Z9 photo takes about 200 ms instead of 710 ms for the first and 240 ms instead of 840 ms for the second. Exporting 500 such photos used about a quarter less processor time. The pictures are unchanged.
