import type { Metadata } from "next";
import type { ReactNode } from "react";
import Link from "next/link";
import Image from "next/image";
import "./globals.css";

export const metadata: Metadata = {
  title: "Nexide - Rust runtime for Next.js",
  description:
    "Showcase of a real Next.js 16 application served by the Nexide Rust + V8 runtime, with a prerender hot path bypassing Node.js entirely.",
};

const NAV_LINKS: ReadonlyArray<{ href: string; label: string }> = [
  { href: "/", label: "Home" },
  { href: "/about", label: "About" },
  { href: "/posts", label: "Posts" },
  { href: "/users", label: "Users" },
  { href: "/forms", label: "Forms" },
];

export default function RootLayout({
  children,
}: Readonly<{ children: ReactNode }>) {
  return (
    <html lang="en">
      <body className="flex min-h-screen flex-col antialiased">
        <header className="sticky top-0 z-10 border-b border-white/5 bg-ink/70 backdrop-blur">
          <nav className="mx-auto flex max-w-5xl items-center justify-between gap-6 px-6 py-4">
            <Link
              href="/"
              className="flex items-center gap-2 text-sm font-semibold tracking-wide text-white"
            >
              <Image
                src="/nexide.png"
                alt="Nexide logo"
                width={89}
                height={28}
                priority
                preload
              />
            </Link>
            <ul className="flex items-center gap-1 text-sm text-white/70">
              {NAV_LINKS.map((link) => (
                <li key={link.href}>
                  <Link
                    href={link.href}
                    className="rounded-md px-3 py-1.5 transition hover:bg-white/5 hover:text-white"
                  >
                    {link.label}
                  </Link>
                </li>
              ))}
            </ul>
          </nav>
        </header>
        <main className="mx-auto w-full max-w-5xl flex-1 px-6 py-12">
          {children}
        </main>
        <footer className="border-t border-white/5 py-6 text-center text-xs text-white/40">
          Served by <span className="font-semibold text-white/70">Nexide</span>{" "}
          - a Rust + V8 runtime for Next.js. No Node.js, no Deno.
        </footer>
      </body>
    </html>
  );
}
