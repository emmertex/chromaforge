### Import → Copy waits for the copy's folder entry
- A copy made by Import → Copy now counts as imported only once its name in the folder is on disk, as a move's already did, so a card wiped after the import can't lose a copy to a crash or power cut. A copy whose folder can't be made durable is reported as failed and left out.
