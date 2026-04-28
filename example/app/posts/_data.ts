/**
 * Shared post fixtures used by the posts index page and the
 * `/posts/[slug]` ISR routes. Co-located in a leaf module so both
 * pages can import without crossing the route segment boundary.
 */
export interface Post {
  slug: string;
  title: string;
  body: string;
  author: string;
  readTimeMinutes: number;
}

export const POSTS: ReadonlyArray<Post> = [
  {
    slug: "hello-world",
    title: "Hello, World!",
    body: "First post served from an ISR-cached static render. Re-fetched on demand by the Rust runtime once the 60-second window elapses.",
    author: "nexide team",
    readTimeMinutes: 2,
  },
  {
    slug: "nexide-rocks",
    title: "nexide Rocks",
    body: "Cached for 60 seconds via Next.js incremental static regeneration. The same prerender hot path serves both static and ISR pages identically.",
    author: "nexide team",
    readTimeMinutes: 4,
  },
];

/** Returns the post with the given slug or `undefined` when unknown. */
export function findPost(slug: string): Post | undefined {
  return POSTS.find((post) => post.slug === slug);
}
