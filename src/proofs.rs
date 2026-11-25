//! Specifications and proof lemmas for the pane split rules.
//!
//! Everything in this file is erased when building with the normal
//! toolchain; it participates in verification only (`scripts/verus-verify.sh`).

use verus_builtin_macros::verus;
// Used only by the `verus!` block below; erased builds see no vstd items.
#[allow(unused_imports)]
use vstd::prelude::*;

/// A pane's split-relevant signature: `(declared id, from)`.
pub type PaneSig = (Option<String>, Option<String>);

verus! {

/// The characters of a signature's declared id, or the empty sequence when
/// no id is declared.
pub open spec fn sig_id_chars(p: PaneSig) -> Seq<char> {
    match p.0 {
        Some(id) => id@,
        None => Seq::empty(),
    }
}

/// The characters of a signature's `from`, or the empty sequence when unset.
pub open spec fn sig_from_chars(p: PaneSig) -> Seq<char> {
    match p.1 {
        Some(f) => f@,
        None => Seq::empty(),
    }
}

/// A pane declared a non-empty id.
pub open spec fn declares_nonempty_id(sigs: Seq<PaneSig>, j: int) -> bool {
    sigs[j].0 is Some && sig_id_chars(sigs[j]).len() > 0
}

/// The non-empty ids declared by `sigs[0..i]`, first occurrence only, in
/// pane order.
pub open spec fn declared_ids(sigs: Seq<PaneSig>, i: int) -> Seq<String>
    decreases i,
{
    if i <= 0 {
        Seq::empty()
    } else {
        let rest = declared_ids(sigs, i - 1);
        match sigs[i - 1].0 {
            Some(id)
                if id@.len() > 0
                    && !exists|k: int| 0 <= k < rest.len() && rest[k]@ == id@ =>
                rest.push(id),
            _ => rest,
        }
    }
}

/// Pane `i`'s `from` names an id declared on an earlier pane of the sequence.
pub open spec fn from_references_earlier(sigs: Seq<PaneSig>, i: int) -> bool {
    sigs[i].1 is Some
        && exists|j: int|
            0 <= j < i && declares_nonempty_id(sigs, j)
                && sig_id_chars(sigs[j]) == sig_from_chars(sigs[i])
}

/// The pane split rules up to (exclusive of) pane `i`: the first pane is not
/// a split, `from` values name an earlier pane's id, and ids are non-empty
/// and pairwise distinct. This is the contract the planner's split-source
/// resolution relies on.
pub open spec fn split_rules_ok_up_to(sigs: Seq<PaneSig>, i: int) -> bool {
    &&& forall|p: int| 0 <= p < i && p == 0 ==> sigs[p].1 is None
    &&& forall|p: int|
        0 < p < i && sigs[p].1 is Some ==> from_references_earlier(sigs, p)
    &&& forall|p: int| 0 <= p < i && sigs[p].0 is Some ==> declares_nonempty_id(sigs, p)
    &&& forall|p: int, q: int|
        0 <= q < p < i && sigs[p].0 is Some && sigs[q].0 is Some
            ==> sig_id_chars(sigs[p]) != sig_id_chars(sigs[q])
}

/// The pane split rules over a whole tab.
pub open spec fn split_rules_ok(sigs: Seq<PaneSig>) -> bool {
    split_rules_ok_up_to(sigs, sigs.len() as int)
}

/// A non-empty id declared by some pane before `i` appears in
/// `declared_ids(sigs, i)`.
pub proof fn lemma_declared_ids_member(sigs: Seq<PaneSig>, i: int, j: int)
    requires
        0 <= j,
        j < i,
        i <= sigs.len(),
        declares_nonempty_id(sigs, j),
    ensures
        exists|k: int|
            0 <= k < declared_ids(sigs, i).len()
                && declared_ids(sigs, i)[k]@ == sig_id_chars(sigs[j]),
    decreases i,
{
    if i == j + 1 {
        let rest = declared_ids(sigs, i - 1);
        if exists|k: int| 0 <= k < rest.len() && rest[k]@ == sig_id_chars(sigs[j]) {
            let k = choose|k: int| 0 <= k < rest.len() && rest[k]@ == sig_id_chars(sigs[j]);
            assert(declared_ids(sigs, i)[k] == rest[k]);
        } else {
            assert(declared_ids(sigs, i).len() == rest.len() + 1);
            assert(declared_ids(sigs, i)[rest.len() as int]@ == sig_id_chars(sigs[j]));
        }
    } else {
        lemma_declared_ids_member(sigs, i - 1, j);
        let sub = declared_ids(sigs, i - 1);
        let k = choose|k: int| 0 <= k < sub.len() && sub[k]@ == sig_id_chars(sigs[j]);
        assert(declared_ids(sigs, i)[k] == sub[k]);
    }
}

/// A value in `declared_ids(sigs, i)` is the id of some pane before `i`.
pub proof fn lemma_declared_ids_witness(sigs: Seq<PaneSig>, i: int, v: Seq<char>)
    requires
        0 <= i,
        i <= sigs.len(),
        exists|k: int| 0 <= k < declared_ids(sigs, i).len() && declared_ids(sigs, i)[k]@ == v,
    ensures
        exists|j: int|
            0 <= j < i && declares_nonempty_id(sigs, j) && sig_id_chars(sigs[j]) == v,
    decreases i,
{
    if i > 0 {
        let k = choose|k: int|
            0 <= k < declared_ids(sigs, i).len() && declared_ids(sigs, i)[k]@ == v;
        let rest = declared_ids(sigs, i - 1);
        if k < rest.len() {
            assert(declared_ids(sigs, i)[k] == rest[k]);
            lemma_declared_ids_witness(sigs, i - 1, v);
        } else {
            assert(k == rest.len());
            assert(declares_nonempty_id(sigs, i - 1));
            assert(sig_id_chars(sigs[i - 1]) == v);
        }
    }
}

} // verus!
