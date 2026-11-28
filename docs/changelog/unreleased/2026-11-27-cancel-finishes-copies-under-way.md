## Import

- **Cancel lets a copy under way finish:** a copy still flushing its file to disk, reading it back on the destination, or converting it to DNG is no longer left behind 5 s after Cancel as "outcome unknown" (a file could then appear outside the library). Cancel's 5 s rule now counts silence as the hung-drive rule does: such work holds it for up to ten times the drive's stall time, then 5 s more without progress leaves the copy behind.
