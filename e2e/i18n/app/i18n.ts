import { createInstance, type i18n, type TFunction } from "i18next";
import resourcesToBackend from "i18next-resources-to-backend";
import { i18nConfig } from "../i18nConfig";

export type InitTranslationsResult = {
  i18n: i18n;
  t: TFunction;
};

export async function initTranslations(
  locale: string,
  namespaces: string[] = ["common"],
): Promise<InitTranslationsResult> {
  const instance = createInstance();
  await instance
    .use(
      resourcesToBackend(
        (lang: string, ns: string) => import(`../locales/${lang}/${ns}.json`),
      ),
    )
    .init({
      lng: locale,
      fallbackLng: i18nConfig.defaultLocale,
      supportedLngs: i18nConfig.locales,
      defaultNS: namespaces[0],
      ns: namespaces,
      interpolation: { escapeValue: false },
    });
  return { i18n: instance, t: instance.getFixedT(locale, namespaces) };
}
