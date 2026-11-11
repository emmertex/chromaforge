## Library

- **Culling suggestions (issue #489).** Assisted Culling suggests a 1–5 star rating and a Pick / Reject flag per photo from its focus, exposure and quality scores, and the dialog can apply them to photos you have not rated or flagged yourself (one undo step). The suggested stars are stored with the photo and filterable as a smart-collection rule field ("Suggested Stars"); `photo.analyze` reports every suggestion (`suggestedStars`, `suggestedFlag`) even without applying it. PR #1105.
