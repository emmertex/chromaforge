## Performance

- **Confirming a hot pixel rebuilds one camera's previews:** preview, thumbnail and view cache keys carried one generation number for the whole process, so confirming one hot pixel made every preview in the library miss; the keys now carry a number for the corrections that apply to the photo's own camera body, so only that body's previews are rebuilt and previews already on disk stay valid (a small baseline file beside the body maps remembers the corrections in use at the switch). Thanks @OneSlips. PR #1078.
