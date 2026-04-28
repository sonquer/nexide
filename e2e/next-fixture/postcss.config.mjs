/**
 * PostCSS pipeline used by Next.js to compile Tailwind v4 utilities.
 * Tailwind ships its own PostCSS plugin (`@tailwindcss/postcss`) that
 * handles `@import "tailwindcss"` in the global stylesheet.
 */
const config = {
  plugins: {
    "@tailwindcss/postcss": {},
  },
};

export default config;
