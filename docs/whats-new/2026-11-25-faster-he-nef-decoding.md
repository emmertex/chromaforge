### Faster Nikon High Efficiency decoding
- Raws in Nikon's High Efficiency and High Efficiency★ formats (Z 9, Z6 III and other recent bodies) decode about 25 to 30 % faster (issue #1377). The step that undoes Nikon's colour-plane transform is about 9 times faster on one thread, and a decode no longer allocates six 45 MB planes per photo. The pictures are unchanged.
