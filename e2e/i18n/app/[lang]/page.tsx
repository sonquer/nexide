import Link from "next/link";
import { initTranslations } from "../i18n";

export const dynamic = "force-dynamic";

export default async function LangIndex({
  params,
}: {
  params: Promise<{ lang: string }>;
}) {
  const { lang } = await params;
  const { t } = await initTranslations(lang);
  const value = (1234567.89).toLocaleString(lang);
  return (
    <main>
      <h1 data-testid="title">{t("title")}</h1>
      <p data-testid="greeting">{t("greeting", { name: "świat" })}</p>
      <p data-testid="intro">{t("intro")}</p>
      <p data-testid="marker">{t("polish_marker")}</p>
      <p data-testid="amount">{t("amount", { value })}</p>
      <ul>
        <li>
          <Link href={`/${lang}/utf8`}>{t("links.utf8")}</Link>
        </li>
        <li>
          <Link href={`/${lang}/intl`}>{t("links.intl")}</Link>
        </li>
        <li>
          <Link href={`/${lang}/static`}>{t("links.static")}</Link>
        </li>
      </ul>
    </main>
  );
}
