---
type: host
of:
  type: object
---

# Host

`of` is `<transform& | object*>`. The inline record claims `object`, the `*`
branch — which is reference-only, so an inline value satisfies no branch. Only a
`transform&` inline record, or an `object*` `[[wikilink]]`, would fit. The
verdict is one `inline-value-type-not-compatible`, never a descent into object's
own field validation (object's required `f` is omitted, so a wrongful descent
would surface `required-field-absent`).
