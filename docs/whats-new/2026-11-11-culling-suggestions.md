### Culling suggestions
- Assisted Culling now suggests a 1–5 star rating and a Pick / Reject flag for every photo, from its focus, exposure and quality scores (issue #489).
- Tick "Suggest star ratings" or "Suggest Pick and Reject flags" in the Assisted Culling dialog: photos you have not rated or flagged yourself get the suggestion; yours are never overwritten.
- The suggested stars stay on the photo (a "Suggested Stars" smart-collection rule field filters by them), and every suggestion is in `photo.analyze`'s answer even without applying it.
