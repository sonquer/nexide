import { initTranslations } from "../../i18n";

export const dynamic = "force-dynamic";

export default async function Utf8Page({
  params,
}: {
  params: Promise<{ lang: string }>;
}) {
  const { lang } = await params;
  const { t } = await initTranslations(lang);
  const items = [
    "Zażółć gęślą jaźń",
    "Mężny bądź, chroń pułk twój i sześć flag",
    "Pchnąć w tę łódź jeża lub ośm skrzyń fig",
    "ąćęłńóśźż",
    "—————————————————————————————————————————————————————————",
  ];
  return (
    <main>
      <h1>{t("links.utf8")}</h1>
      <p data-testid="marker">{t("polish_marker")}</p>
      <ul>
        {items.map((line, i) => (
          <li key={i}>{line}</li>
        ))}
      </ul>
    </main>
  );
}
