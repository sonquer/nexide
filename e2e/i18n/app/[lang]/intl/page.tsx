import { initTranslations } from "../../i18n";

export const dynamic = "force-dynamic";

export default async function IntlPage({
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
      <h1>{t("links.intl")}</h1>
      <p data-testid="default-locale">default={value.toLocaleString()}</p>
      <p data-testid="lang-locale">
        {lang}={value.toLocaleString(lang)}
      </p>
      <p data-testid="en-us">
        en-US={new Intl.NumberFormat("en-US").format(value)}
      </p>
      <p data-testid="pl-pl">
        pl-PL={new Intl.NumberFormat("pl-PL").format(value)}
      </p>
      <p data-testid="de-de">
        de-DE={new Intl.NumberFormat("de-DE").format(value)}
      </p>
      <p data-testid="dt-pl">
        dt-pl=
        {new Intl.DateTimeFormat("pl-PL", { dateStyle: "long" }).format(date)}
      </p>
    </main>
  );
}
