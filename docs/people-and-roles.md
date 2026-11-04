# People and roles in masks

Chromaforge's People masks can say *who* they are about, so a mask made on one photo follows the same person on
the next. Ported from OneSlips' fork.

- **By name:** `{"kind": "people", "parts": ["Clothes"], "who": {"name": "Alex"}}` selects the person whose face
  is named Alex on the photo. People are counted from the left by their faces, as the People mask counts them.
- **By role:** `"who": {"role": "Bride"}` uses the person bound to the role for that photo. `people.setRole
  {role, person, scope}` binds it to a folder (the default: the photo's folder and its sub-folders), an album, a
  date range or a list of photos; the most specific binding wins. `people.roles` lists them, `people.clearRole`
  removes them.
- **Anchored gradients:** a component's `anchor` (`{"to": "people" | "subject" | {"who": …}, "shape": …}`) places
  a linear or radial gradient in the box of the people (or one person) on each photo, estimated from their faces.
- **When:** presets, Paste, Paste from Previous and Sync complete these per photo. `masks.recompute {id | ids,
  force?}` does it again and also makes Object and Describe selections for the photo they are on (with the SAM
  model installed); nobody in a role, or a name not on the photo, means everyone, and the result says so.
- **In the app:** select a People component in the Masking panel: *Who* picks everyone, a named person or a role;
  *Set Role…* binds a role for this folder.

## Privacy

Roles and appearance examples live only in the library's `people.json`; they are never written to sidecars, XMP,
exports or the network. `people.forget {person}` and `people.forgetAll` delete them. Face names stay on the photos
and face recognition data under `faces.*`.

## Not ported yet

Recognising a person by appearance (clothes, hair, face proportions) without a named face, remembered objects
("concepts"), SMART-5's per-person silhouettes where people overlap, and the Masking panel's thumbnail tree.
