### A SAM 3 model in the wrong format says so
- Object and Describe need `model.safetensors` from `facebook/sam3` in the Hugging Face format. The
  `sam3.safetensors` on community mirrors is Meta's original checkpoint, with other tensor names; installed in its
  place, AI masks failed with "unsupported dtype C64". The app now says that the file is Meta's original
  checkpoint and which file it needs instead.