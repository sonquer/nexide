import { i18nConfig } from "../../../i18nConfig";
import { initTranslations } from "../../i18n";

export function generateStaticParams() {
  return i18nConfig.locales.map((lang) => ({ lang }));
}

export default async function StaticPage({
  params,
}: {
  params: Promise<{ lang: string }>;
}) {
  const { lang } = await params;
  const { t } = await initTranslations(lang);
  const value = 1234567.89;
  const date = new Date(Date.UTC(2024, 0, 15, 12, 0, 0));
  return (
    <main>
      <h1>{t("links.static")}</h1>
      <p data-testid="static-marker">{t("polish_marker")}</p>
      <p data-testid="static-greeting">{t("greeting", { name: "świat" })}</p>
      <p data-testid="static-pl">
        pl-PL={new Intl.NumberFormat("pl-PL").format(value)}
      </p>
      <p data-testid="static-de">
        de-DE={new Intl.NumberFormat("de-DE").format(value)}
      </p>
      <p data-testid="static-date">
        date=
        {new Intl.DateTimeFormat("pl-PL", { dateStyle: "long" }).format(date)}
      </p>
      <ul>
        <li>Zażółć gęślą jaźń</li>
        <li>Mężny bądź, chroń pułk twój i sześć flag</li>
        <li>Pchnąć w tę łódź jeża lub ośm skrzyń fig</li>
      </ul>
    </main>
  );
}
