import { NextResponse } from "next/server";

export function GET() {
  const value = 1234567.89;
  const fixedDate = new Date(Date.UTC(2024, 0, 15, 12, 0, 0));

  const noArgs = value.toLocaleString();
  const noArgsDate = fixedDate.toLocaleDateString();

  const explicit = {
    "en-US": new Intl.NumberFormat("en-US").format(value),
    "de-DE": new Intl.NumberFormat("de-DE").format(value),
    "pl-PL": new Intl.NumberFormat("pl-PL").format(value),
    "ja-JP": new Intl.NumberFormat("ja-JP").format(value),
  };

  const dates = {
    "en-US": new Intl.DateTimeFormat("en-US", { dateStyle: "long" }).format(
      fixedDate,
    ),
    "pl-PL": new Intl.DateTimeFormat("pl-PL", { dateStyle: "long" }).format(
      fixedDate,
    ),
  };

  const collator = new Intl.Collator("pl-PL").compare("ą", "b");

  return NextResponse.json({
    runtime: "nexide",
    noArgs,
    noArgsDate,
    explicit,
    dates,
    collator,
    polish: "Zażółć gęślą jaźń — pchnąć w tę łódź jeża",
  });
}
