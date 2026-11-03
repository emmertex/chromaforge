# People corpus: files and test hooks

`cargo xtask corpus --download` now also fetches `PEOPLE_SAMPLES` (xtask/src/main.rs) into the gitignored
`corpus/people/`, checking SHA-256 and byte size. All from Wikimedia Commons; licence templates checked on each file page.

| file | covers | Commons page | licence |
|---|---|---|---|
| pd-one-lincoln-1863.jpg | 1 person, frontal | https://commons.wikimedia.org/wiki/File:Abraham_Lincoln_O-77_matte_collodion_print.jpg | PD (PD-scan / PD-old-auto-1923), Alexander Gardner |
| pd-two-curie-1895.jpg | 2 people close together | https://commons.wikimedia.org/wiki/File:Pierre_Curie_et_Marie_Sklodowska_Curie_1895.jpg | PD (PD-old-auto, PD-France, PD-1996), unknown author |
| pd-group-solvay-1927.jpg | group of 29, overlapping rows, glasses | https://commons.wikimedia.org/wiki/File:Solvay_conference_1927.jpg | PD (PD-because, Belgium >70 y), Benjamin Couprie |
| pd-profile-edison.jpg | profile view | https://commons.wikimedia.org/wiki/File:Portrait_of_Thomas_Edison_in_profile_facing_right._(9e5a2edcde9346628e2acf7d63733576).jpg | PD (PD-US-expired) |
| pd-glasses-gandhi-1931.jpg | glasses | https://commons.wikimedia.org/wiki/File:Mahatma-Gandhi,_studio,_1931.jpg | PD (PD-UK-unknown), Elliott & Fry |
| pd-eyes-closed-fitz-1839.jpg | closed eyes | https://commons.wikimedia.org/wiki/File:Henry_Fitz_Jr._The_Eyes_Closed.jpg | PD (PD-old-70), Henry Fitz Jr. self-portrait |

All are greyscale/sepia historical prints (no colour portrait was found that is CC0 with the photographer as uploader
and no private individual); colour-dependent checks (skin, lips) should stay lenient on them.

## Test hooks to wire after the masks merge (port/oneslips-masks-people)

The real-model tests today run on `Session::with_demo()` (no faces), so they only prove "no error". With the corpus:

- `crates/engine/src/tests_people_masks.rs::people_masks_run_on_the_real_models` — import each `corpus/people/*.jpg`
  (skip `Corpus` when absent, `Model` without `<PREFIX>_MEDIAPIPE_ROOT`); `mask.add {"kind":"people"}` must give a
  non-empty mask; on Solvay/Curie, per-person masks (`people` with a person index) must be several and mostly disjoint;
  `parts: ["Lips","Teeth"]` must not error on profile / closed eyes.
- SMART-5 (the per-person silhouettes in the same file / `lcc_develop::MAX_PEOPLE_PARTS`): count of persons found
  ≥ 1 on single portraits, = 2 on Curie, ≥ 10 on Solvay; no panic or error on profile/closed eyes/glasses.
- `crates/engine/src/identity.rs::learning_runs_with_the_real_models` — `people.learn` on the corpus photos: faces
  found (`known_appearance` non-empty) on Lincoln, Gandhi, Curie; naming one Solvay face and `find_by_look` on a
  second import of the same file finds it again.
- Helper suggestion: `fn people_corpus(name) -> Option<PathBuf>` = `<repo>/corpus/people/<name>` if it exists, else
  `lcc_testkit::skip!(Corpus, "{name} absent (cargo xtask corpus --download)")`.

Assertions should be behavioural (counts within ranges, non-empty, disjointness), not pixel goldens.
