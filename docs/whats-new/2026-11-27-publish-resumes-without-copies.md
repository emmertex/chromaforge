### An interrupted publish leaves no duplicates
- After a Hard Drive publish was stopped partway (the app closed, a crash), the next publish recognises the files already sent instead of writing a second copy of each as `name-2.jpg` (issue #1376).
