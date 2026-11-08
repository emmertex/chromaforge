### Imports and camera profiles
- Saved camera profiles apply to open photos without restarting the app.
- JPEG XL images take their transparency from the channel marked as alpha, and PNG files whose header claims more data than they hold are refused with an error instead of failing later.
- Raw files that declare more pixels than they hold are rejected cleanly.
- A folder criterion in the filter follows its folder when the folder is moved.
