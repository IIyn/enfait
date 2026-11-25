## One part of the video

The video is long: you write **one part** of it, the blocks under "Blocks
to write". The other parts are written separately, with the same
instructions. For the flow you get, when there is one, the block just
before as it was written (and its transition into this part, if it is
crafted) and the text of the block just after. Block numbers are those of
the whole video. The transition of the last block of the part leads to the
block just after. Crafted transitions left for the whole video are given
in the message: when none are left, every transition is `cut`.

## Answer

First think in a few lines: what this part says, its place in the video,
where its strong moments go. Then finish with a single ```` ```json ````
block that follows exactly this schema:

```json
{
  "blocks": [
    {
      "index": 12,
      "kind": "animation",
      "description": "what we see",
      "moments": [ { "word": "exact word of the block", "note": "what happens" } ],
      "broll": null,
      "transition": { "type": "cut", "description": "" },
      "sfx": [ { "word": "exact word of the block", "query": "whoosh" } ]
    },
    {
      "index": 13,
      "kind": "broll",
      "description": "wide shot of a station at night",
      "moments": [],
      "broll": { "query": "train station night", "media": "video" },
      "transition": { "type": "auto", "description": "" },
      "sfx": []
    }
  ]{{MUSIC_SCHEMA}}
}
```

One object per block to write, in order, with its number as `index`.
Transition `type`: `cut`, `auto` or `described`. The `word` values are
copied as they appear in the text of the block. Descriptions and notes are
written in {{LANGUAGE}}; queries in English.
