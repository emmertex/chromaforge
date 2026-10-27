### Floating-point DNGs store their white level as the DNG specification asks
- HDR merges and other floating-point DNGs wrote `WhiteLevel` as a fraction, a type the DNG specification doesn't
  allow, so stricter DNG readers ignored it. It is now written as a whole number.
