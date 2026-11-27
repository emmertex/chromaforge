## Develop

- **Highlight reconstruction (Rebuild):** a partly clipped area with no reliable colour nearby now takes its own colour from just below the clip (as far as its unclipped channels agree) instead of a neutral guess. With green and blue clipped and red not, the neutral guess lifted green to blue's level, so large bright skies rendered cyan once Exposure or Highlights brought them below white (#1487).
