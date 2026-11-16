//! The desktop app runs on the Windows Segment Heap (`build.rs` → `MANIFEST`): on the default heap
//! the render workers' large allocations held the one heap lock the window's thread needs, and the
//! interface froze for 50–300 ms at a time while Build Previews or an export ran.

#[cfg(windows)]
#[test]
fn the_app_asks_windows_for_the_segment_heap() {
    let exe = std::fs::read(env!("CARGO_BIN_EXE_app")).expect("the app's executable");
    let wanted: &[u8] = b"<heapType xmlns=\"http://schemas.microsoft.com/SMI/2020/WindowsSettings\">SegmentHeap</heapType>";
    assert!(exe.windows(wanted.len()).any(|w| w == wanted), "no Segment Heap manifest in the executable");
}
