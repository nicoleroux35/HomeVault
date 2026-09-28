import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
I18N_DIR = ROOT / "src" / "i18n"
OUTPUT = ROOT / "static" / "js" / "translations.js"

SECTION_RE = re.compile(r"^\s*\[([^\]]+)\]\s*$")
KV_RE = re.compile(r'^\s*([A-Za-z0-9_.-]+)\s*=\s*"(.*)"\s*$')


def parse_toml_like(path: Path) -> dict[str, str]:
    data: dict[str, str] = {}
    current: list[str] = []

    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue

        section_match = SECTION_RE.match(line)
        if section_match:
            current = section_match.group(1).split(".")
            continue

        kv_match = KV_RE.match(raw_line)
        if kv_match:
            key = kv_match.group(1).strip()
            value = kv_match.group(2)
            value = value.replace(r'\"', '"')
            value = value.replace(r"\n", "\n")
            full_key = ".".join([*current, key]) if current else key
            data[full_key] = value

    return data


def build_translation_map() -> dict[str, dict[str, str]]:
    translations: dict[str, dict[str, str]] = {}
    for path in sorted(I18N_DIR.glob("*.toml")):
        translations[path.stem] = parse_toml_like(path)
    return translations


def main() -> None:
    translations = build_translation_map()
    payload = json.dumps(translations, ensure_ascii=False, indent=2)
    OUTPUT.write_text(
        "const translations = " + payload + ";\n\n"
        "const I18n = {\n"
        "  currentLang: localStorage.getItem('language') || 'en',\n\n"
        "  t: (key, fallback = '', params = {}) => {\n"
        "    let text = translations[I18n.currentLang]?.[key] || translations['en']?.[key] || fallback || key;\n"
        "    Object.entries(params).forEach(([k, v]) => {\n"
        "      text = text.replace(new RegExp(`%\\{${k}\\}`, 'g'), v);\n"
        "    });\n"
        "    return text;\n"
        "  },\n\n"
        "  setLanguage: (lang) => {\n"
        "    I18n.currentLang = lang;\n"
        "    localStorage.setItem('language', lang);\n"
        "    document.documentElement.lang = lang;\n"
        "    I18n.applyTranslations();\n"
        "  },\n\n"
        "  applyTranslations: () => {\n"
        "    document.querySelectorAll('[data-i18n]').forEach((element) => {\n"
        "      const key = element.getAttribute('data-i18n');\n"
        "      const translation = I18n.t(key, element.textContent?.trim() || '');\n"
        "      if (element.tagName === 'INPUT' || element.tagName === 'TEXTAREA') {\n"
        "        if (element.hasAttribute('placeholder')) {\n"
        "          element.setAttribute('placeholder', translation);\n"
        "        } else {\n"
        "          element.value = translation;\n"
        "        }\n"
        "      } else if (element.tagName === 'OPTION') {\n"
        "        element.textContent = translation;\n"
        "      } else {\n"
        "        element.textContent = translation;\n"
        "      }\n"
        "    });\n"
        "  }\n"
        "};\n\n"
        "document.addEventListener('DOMContentLoaded', () => {\n"
        "  document.documentElement.lang = I18n.currentLang;\n"
        "  I18n.applyTranslations();\n"
        "});\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
