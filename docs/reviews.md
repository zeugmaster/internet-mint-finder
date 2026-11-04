# Review event contract

IMF writes [NIP-87](https://github.com/nostr-protocol/nips/blob/master/87.md)
kind 38000 reviews and reads kind 38172 Cashu announcements. Signing is through
[NIP-07](https://github.com/nostr-protocol/nips/blob/master/07.md). Addressable-event
replacement and timestamp ties follow
[NIP-01](https://github.com/nostr-protocol/nips/blob/master/01.md).

## New IMF reviews

```json
{
  "kind": 38000,
  "created_at": 1790000000,
  "tags": [
    ["k", "38172"],
    ["d", "<mint public key, or canonical URL when unknown>"],
    ["u", "https://mint.example"],
    ["a", "38172:<announcer public key>:<mint public key>"]
  ],
  "content": "[4/5] My experience"
}
```

The extension adds `pubkey`, `id` and `sig`. The `a` tag is omitted when its
announcement address is unknown. URL fallback preserves interoperability with
legacy URL-addressed reviews without blocking reviews of unannounced mints.
The application always supplies `k`; historical readers accept its absence when
the event otherwise references a Cashu URL, a mint key or a Cashu `a` address.

## Formats covered by tests

| Input | Treatment |
| --- | --- |
| `38000`, `k=38172`, `d=mint key`, `u=URL` | Current IMF/NIP-87 review |
| `38000`, `d=URL`, no `k` or `u` | Legacy URL-addressed review |
| `38000`, typed `u=URL,cashu`, no `k`/`d` | Legacy recommendation |
| `38000`, one or more Cashu `a` addresses | Attach to the specifically referenced announcement, including when it arrives later |
| Multiple Cashu `u` tags | Apply to each referenced mint, count the author once per mint |
| `38000`, `k=38173` only, or typed Fedimint URLs | Ignore Fedimint recommendations |
| `[0/5]` through `[5/5]` | Parse rating; browser composition offers 1–5 stars |
| Text without a valid rating prefix | Display text; exclude from the star average |
| Updated URL-addressed and pubkey-addressed reviews by the same author | Only the latest contributes to the mint's rating/count |
| Same event from several relays | Count once |
| Equal timestamps at the same address | Lowest event ID wins, independent of arrival order |
| Invalid signatures, future timestamps over 60 seconds, events over 64 KiB, unsafe/non-HTTP URLs | Reject/ignore |

These are explicit supported shapes, not a promise to understand every historical
client. Unknown event kinds are ignored rather than interpreted as reviews. NIP-87
has no standard rating tag or JSON rating schema; IMF emits the established `[N/5]`
content prefix. The previous IMF writer already used kind 38000; strict tag and
identity handling, rather than a new kind number, caused the compatibility gaps.

Raw signed events are retained alongside the projection. Replacement is keyed by
`(kind, author, d)`; missing `d` has the NIP-01 empty-string value. The rating view
then selects the latest event per `(mint URL, author)` across legacy and current
identifiers. Stats and review lists use that same view. A review containing neither
text nor rating can replace an old recommendation without contributing a score.

Metadata from announcements is untrusted and described as published on Nostr.
IMF does not verify that the announcement author owns the mint or fetch NUT-06
information in phase one. Public keys and URLs are references, not endorsements.

## Relay limitations

The replay worker requires EOSE before accepting a page. It overlaps boundary
seconds, expands saturated pages up to 20,000 events, and retains the cursor if
that limit is reached. Nostr pagination has no per-event cursor: a relay silently
capping results below the requested limit can still hide events at a saturated
timestamp. Configuring multiple useful relays reduces source gaps but is not
application replica infrastructure. Unavailable or permanently rejecting relays
leave local submissions visibly pending; operators can change `IMF_RELAYS`.
