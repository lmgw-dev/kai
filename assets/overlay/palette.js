// The ring's colours: lmgw's voice tokens (crates/lmgw-ui/assets/voice.css, `--rt-*`, mapped to
// the engine's roles as the Chat's `palette()` maps them): graphite and blue, amber for the user.
// `bg` is never drawn (the ring is transparent) but shades the engine's derived colours.
// `scripts/viz-adapter-check.mjs` compares these with lmgw's file.
export const PALETTE = {
  bg: "#121518",
  accent: { deep: "#0F4F7A", base: "#3DAEE9", hi: "#C4E9FB" },
  accent2: { deep: "#163E5A", base: "#5AA9D6", hi: "#CFE6F4" },
  muted: { deep: "#1E2A35", base: "#4F6476", hi: "#9DB0C0" },
  warn: { deep: "#6E420C", base: "#E8A33D", hi: "#FFE0AE" },
};
