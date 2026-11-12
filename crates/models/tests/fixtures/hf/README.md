# Hugging Face answer fixtures

Made-up answers in the shape of `GET /api/models/<repo>?blobs=true&expand[]=…` (the
fields `HfClient::repo` asks for). Every name, number and hash is invented; they exist
so the parsing of what an uploader lists (0.0.6 item 4) can be tested without the
network. `repo-hostile.json` holds the kinds of text an uploader could use to mislead:
a right-to-left override, zero-width characters, markup, over-long text and links that
are not https.
