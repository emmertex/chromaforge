### Clearer refusals from the command-line tool and develop controls
- The command-line tool refuses an argument that isn't valid UTF-8 with a message naming it, instead of stopping.
- A `run --script` line whose `params` isn't a JSON object is refused before any command runs (it used to run with no parameters).
- A develop control id with a signed list index (`pointColor.+0.hueShift`) is an unknown id, as a typo should be.
