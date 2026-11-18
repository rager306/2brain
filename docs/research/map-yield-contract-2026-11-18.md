# The map-yield contract: the code and its sentence disagree, and the code wins

**Question.** What is the exact rule for demoting a `type: moc` hit, and why does the sentence that states it say
the opposite of what `yield_maps` does?

**What the sentence says.** Three places state it the same way: the doc comment above the function, the commit
that introduced it (`1d42e02`), and the registration in [`task-set.md`](task-set.md) §4 — "a `type: moc` hit is
halved when a non-map hit matched at least as many terms". Read as a condition on the map, that is
`map_terms <= best_note_terms`.

**What the code does.** `hit.matched_terms >= best`, where `best` is the maximum `matched_terms` among the
page's non-map hits. The map is halved when it matched **at least as many** terms as any note — the opposite
inequality.

**Why no measurement caught it.** The case the rule was registered for is a tie. `task-set.md` describes it
itself: "the remaining gap is the `type: moc` map that matches the same six terms". When map and note match the
same number, `>=` and `<=` agree, so `hub-vs-atomic 8 → 1` cannot distinguish the two directions. Every other
class was unchanged, which is also consistent with both.

**Which direction is right.** The useful failure is the multi-term one: a query of three or more terms where a
broad map matches more terms than the atomic note that answers it. Only `map >= best` discounts that map; the
sentence's `<=` leaves it on top, which is the defect the rule was written to remove. The opposite case — a map
that matched fewer terms than a note — is already ranked below that note on terms alone, so discounting it
changes little and declining to discount it changes nothing. The code's direction is therefore the one with a
reason, and the sentence is the part that is wrong.

**Resolution.** `serve.rs` states the rule as implemented, `map_yields(map_terms, best)` holds it as one
function, and `map_yield_tests` pins both unequal cases so a sign flip fails a test instead of silently
re-ranking. `task-set.md` is a dated research note and is not edited; this note is the correction it points to.

**What is not claimed.** No run in this repository distinguishes `>=` from `<=`, because every registered case
was a tie. A future change of direction needs a query where the map and the best note match a different number
of terms, measured on the task set — a reading of the code is not a measurement.

**Found by** the external review of `93bcaae` (2026-11-18), which read the condition against the contract.
