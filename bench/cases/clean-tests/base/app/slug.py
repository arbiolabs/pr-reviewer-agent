import re
import unicodedata


def slugify(text: str) -> str:
    """Lowercase ASCII words joined by single hyphens."""
    ascii_text = unicodedata.normalize("NFKD", text).encode("ascii", "ignore").decode("ascii")
    words = re.findall(r"[a-z0-9]+", ascii_text.lower())
    return "-".join(words)
