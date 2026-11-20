// Planted by the kani-ledger-unlisted control: a harness that narrows its
// input with an assumption the ledger does not list.
#[kani::proof]
fn zz_plant_harness() {
    let x: u8 = kani::any();
    kani::assume(x < 8);
    assert!(x < 9);
}
