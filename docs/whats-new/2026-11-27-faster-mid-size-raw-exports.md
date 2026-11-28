### Faster mid-size exports from large raws
- A raw exported at up to about half its sensor's size (about 3,500 px from a 45 MP camera) is read at half resolution and resized once to the output, instead of reading the whole sensor: a 3,000 px JPEG from 45 MP takes about half the time and memory (issue #1377).
- Exports of 2,560 px and smaller from raws are sharper: their fine detail no longer goes through an extra soft resize step.
