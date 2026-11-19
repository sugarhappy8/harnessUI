# 0.1.0: the cloud update

The owner asked (2026-11-17), word for word:

> For the cloud update let's make 0.1 let's add image and video gen section for those models, make it easy to use but not dumbed down

Decisions the owner made on 2026-11-17:

1. Companies in 0.1.0: **OpenRouter and OpenAI**, reusing the keys already saved in *Settings → Endpoints*.
2. **No picture tool for chat models** in 0.1.0. Chats and Studio connect through *Send to chat* and *Open in Studio*.
3. Spending (cloud runs): confirm every video, any run of $1 or more and any run with an unknown price, plus a **daily limit of $10, on by default**, that pauses Studio until it is raised.

Later the same day the owner asked, word for word:

> I also mean like downloading local models that can generate video.

and made four decisions about local models (recorded at the top of [studio-local-design.md](studio-local-design.md), which they override wherever it disagrees):

1. **Local is the main thing; cloud is the backup.** Local pictures and local video ship in 0.1.0, together with the cloud Studio. Studio leads with "On this computer"; cloud (OpenRouter or OpenAI) is the backup for computers without a good graphics card, suggested when no local model fits or a local run would be very slow. The owner's words: "It should be models u can pick the like hugging face downloads and it should ship with cloud and emphasis on local being the main thing while cloud is backup for those who don't have good gpu's".
2. **People pick local models the way they pick chat models: by searching Hugging Face**, not only from a fixed list. For families harnessUI recognises, the search fills in the extra files those models need and shows the total size, licence and fit before download; families it does not recognise show "Not supported yet" instead of a Download button.
3. **The Windows engine downloads with the first local model.** The Mac bundles it.
4. **The recommended list is free-for-any-use only:** FLUX.2 [klein] 4B, Z-Image Turbo and Wan 2.2 TI2V-5B (fast version), plus Wan 2.1 1.3B if the benchmark calls for it. Models found through the search show their licence in plain words, and the licence must be accepted before download.

| File | What it is |
|---|---|
| [studio-design.md](studio-design.md) | The design: Studio, where it lives, what it shows, how runs, money, the gallery and privacy work, and the technical plan mapped to files |
| [studio-local-design.md](studio-local-design.md) | Addendum: local models in Studio (engine, downloads, fit, licences), with the owner's decisions at the top |
| [studio-plan.md](studio-plan.md) | The implementation plan, task by task |
| [owner-checklist.md](owner-checklist.md) | What only the owner's computers can show: the one-day local benchmark first (no local time is shown or written before it), then the real cloud and local runs |
| [research/providers.md](research/providers.md) | Image and video APIs as of 2026-11-17, with sources |
| [research/ux-patterns.md](research/ux-patterns.md) | How other generation tools stay easy without being dumbed down |
| [research/architecture-map.md](research/architecture-map.md) | Where Studio plugs into harnessUI today |
| research/local-*.md | Local engines and models, licences, and how the existing local runtime can be reused |
| research/catalog/ | OpenRouter's image and video catalogues as fetched on 2026-11-17 (test fixtures) |
