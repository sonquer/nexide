import Link from "next/link";
import { notFound } from "next/navigation";
import { POSTS, findPost } from "../_data";

interface PostPageProps {
  params: Promise<{ slug: string }>;
}

export const revalidate = 60;
export const dynamicParams = false;

export function generateStaticParams() {
  return POSTS.map((post) => ({ slug: post.slug }));
}

export default async function PostPage({ params }: PostPageProps) {
  const { slug } = await params;
  const post = findPost(slug);
  if (!post) {
    notFound();
  }
  return (
    <article
      data-testid="post-page"
      className="mx-auto flex max-w-3xl flex-col gap-8"
    >
      <Link
        href="/posts"
        className="inline-flex w-fit items-center gap-2 text-sm text-white/60 hover:text-white"
      >
        ← All posts
      </Link>
      <header className="flex flex-col gap-3">
        <span className="inline-flex w-fit items-center gap-2 rounded-full border border-white/10 bg-white/[0.03] px-3 py-1 text-xs font-medium text-white/60">
          ISR with revalidate {revalidate} s
        </span>
        <h1
          data-testid="post-title"
          className="text-4xl font-bold tracking-tight text-white"
        >
          {post.title}
        </h1>
        <p className="text-sm text-white/60">
          By {post.author}, {post.readTimeMinutes} min read,{" "}
          <span data-testid="post-slug" className="font-mono">
            {post.slug}
          </span>
        </p>
      </header>
      <p
        data-testid="post-body"
        className="text-lg leading-relaxed text-white/80"
      >
        {post.body}
      </p>
    </article>
  );
}
