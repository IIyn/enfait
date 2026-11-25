## Answer

First think in a few lines: the message of the video, its tone, the place
of the filmed person if there is one, where the strong moments go. Then
finish with a single ```` ```json ```` block that follows exactly this
schema:

```json
{
  "blocks": [
    {
      "index": 1,
      "kind": "animation",
      "description": "what we see",
      "moments": [ { "word": "exact word of the block", "note": "what happens" } ],
      "broll": null,
      "transition": { "type": "cut", "description": "" },
      "sfx": [ { "word": "exact word of the block", "query": "whoosh" } ]
    },
    {
      "index": 2,
      "kind": "broll",
      "description": "wide shot of a station at night",
      "moments": [],
      "broll": { "query": "train station night", "media": "video" },
      "transition": { "type": "auto", "description": "" },
      "sfx": []
    }
  ],
  "music": { "query": "calm piano ambient", "volume_db": -18 }
}
```

One object per block in `blocks`, in order, `index` starting at 1.
Transition `type`: `cut`, `auto` or `described`. The `word` values are
copied as they appear in the text of the block. Descriptions and notes are
written in {{LANGUAGE}}; queries in English.
