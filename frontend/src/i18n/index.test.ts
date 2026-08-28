import { describe, expect, it } from "vitest";
import { resources, displayLanguages } from "./index";

function flatten(
  value: Record<string, unknown>,
  prefix = "",
): Record<string, string> {
  return Object.fromEntries(
    Object.entries(value).flatMap(([key, child]) => {
      const path = prefix ? `${prefix}.${key}` : key;
      return typeof child === "string"
        ? [[path, child]]
        : Object.entries(flatten(child as Record<string, unknown>, path));
    }),
  );
}

function placeholders(value: string): string[] {
  return [...value.matchAll(/\{\{[^{}]+\}\}/g)].map(([match]) => match).sort();
}

describe("i18n resources", () => {
  it("keeps every namespace and translation key aligned across supported languages", () => {
    const referenceNamespaces = Object.keys(resources.en).sort();
    const reference = Object.fromEntries(
      referenceNamespaces.map((namespace) => [
        namespace,
        Object.keys(flatten(resources.en[namespace])).sort(),
      ]),
    );

    for (const language of displayLanguages) {
      expect(Object.keys(resources[language]).sort(), language).toEqual(
        referenceNamespaces,
      );
      for (const namespace of referenceNamespaces) {
        expect(
          Object.keys(flatten(resources[language][namespace])).sort(),
          `${language}/${namespace}`,
        ).toEqual(reference[namespace]);
      }
    }
  });

  it("preserves interpolation placeholders in every translation", () => {
    for (const language of displayLanguages) {
      for (const [namespace, tree] of Object.entries(resources.en)) {
        const reference = flatten(tree);
        const translated = flatten(resources[language][namespace]);
        for (const [key, value] of Object.entries(reference)) {
          expect(
            placeholders(translated[key]),
            `${language}/${namespace}:${key}`,
          ).toEqual(placeholders(value));
        }
      }
    }
  });
});
