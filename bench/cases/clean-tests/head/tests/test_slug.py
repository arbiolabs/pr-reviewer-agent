from app.slug import slugify


def test_joins_words_with_single_hyphens():
    assert slugify("Hello,   World!") == "hello-world"


def test_strips_accents():
    assert slugify("Crème Brûlée") == "creme-brulee"


def test_keeps_digits():
    assert slugify("Top 10 Tips") == "top-10-tips"


def test_is_empty_without_ascii_words():
    assert slugify("!!! ???") == ""
