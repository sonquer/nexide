import type { ReactNode } from "react";
import { i18nConfig } from "../../i18nConfig";

export function generateStaticParams() {
  return i18nConfig.locales.map((lang) => ({ lang }));
}

export default async function LangLayout({
  children,
  params,
}: {
  children: ReactNode;
  params: Promise<{ lang: string }>;
}) {
  const { lang } = await params;
  return (
    <html lang={lang}>
      <body>{children}</body>
    </html>
  );
}
