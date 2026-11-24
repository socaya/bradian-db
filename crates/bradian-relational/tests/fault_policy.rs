//! ADR 0288 — FaultPolicy trait and reference impls.

use bradian_relational::fault_injection::{
    FaultPolicy, FailAfterNWrites, DropOnNthSync, NeverFail,
};
use std::io;

#[test]
fn never_fail_permits_everything() {
    let p = NeverFail;
    assert!(p.on_write().is_ok());
    assert!(p.on_sync().is_ok());
    for _ in 0..1000 {
        assert!(p.on_write().is_ok());
    }
}

#[test]
fn fail_after_n_writes_counts_down_then_fires_once() {
    let p = FailAfterNWrites::new(3);
    assert!(!p.has_fired());
    assert!(p.on_write().is_ok()); // 3 -> 2
    assert!(p.on_write().is_ok()); // 2 -> 1
    assert!(!p.has_fired());
    let err = p.on_write().unwrap_err(); // fires
    assert_eq!(err.kind(), io::ErrorKind::Other);
    assert!(p.has_fired());
    assert!(p.on_write().is_ok()); // post-fire: Ok.
    assert!(p.on_write().is_ok());
}

#[test]
fn drop_on_nth_sync_marks_fired_but_returns_ok() {
    let p = DropOnNthSync::new(2);
    assert!(p.on_sync().is_ok()); // 2 -> 1; not fired
    assert!(!p.has_fired());
    assert!(p.on_sync().is_ok()); // 1 -> 0; fires
    assert!(p.has_fired());
    assert!(p.on_sync().is_ok()); // post-fire still Ok
}

#[test]
fn policies_are_sync_and_send() {
    fn require<T: FaultPolicy + ?Sized>() {}
    require::<NeverFail>();
    require::<FailAfterNWrites>();
    require::<DropOnNthSync>();
}

#[test]
fn concurrent_fire_uses_once_only() {
    use std::sync::Arc;
    let p = Arc::new(FailAfterNWrites::new(50));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let pc = Arc::clone(&p);
        handles.push(std::thread::spawn(move || {
            for _ in 0..20 {
                let _ = pc.on_write();
            }
        }));
    }
    for h in handles { h.join().unwrap(); }
    // Exactly one of the 160 attempts fires; the rest are Ok.
    assert!(p.has_fired());
}
