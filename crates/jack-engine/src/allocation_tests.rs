//! Count allocations on the current test thread only, excluding setup/other tests.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
thread_local! {
    static ACTIVE:Cell<bool> = const {Cell::new(false)};
    static COUNT:Cell<usize> = const {Cell::new(0)};
}
struct CountingAllocator;
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
fn count() {
    if ACTIVE.try_with(Cell::get).unwrap_or(false) {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
    }
}
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[test]
fn mixer_and_resampler_processing_allocate_nothing() {
    use crate::{
        mixer::{DeviceMixer, PcmFormat},
        resampler::StreamingResampler,
    };
    let id = uuid::Uuid::from_u128(1);
    let mut mixer = DeviceMixer::new(
        PcmFormat {
            sample_rate: 48000,
            channels: 2,
            channel_mask: 3,
        },
        480,
        960,
    )
    .unwrap();
    mixer.add_group(id, 1.0).unwrap();
    let mut converter = StreamingResampler::new(44100, 48000, 2, 960).unwrap();
    let input = [0.1; 882];
    let mut converted = [0.0; 960];
    let mut output = [0.0; 960];
    COUNT.with(|c| c.set(0));
    ACTIVE.with(|v| v.set(true));
    for _ in 0..100 {
        converter.push(&input).unwrap();
        let frames = converter.process(&mut converted).unwrap();
        mixer.push(id, &converted[..frames * 2]).unwrap();
        mixer.set_gain(id, 0.5, 240).unwrap();
        mixer.render(&mut output).unwrap();
    }
    ACTIVE.with(|v| v.set(false));
    assert_eq!(COUNT.with(Cell::get), 0);
}
