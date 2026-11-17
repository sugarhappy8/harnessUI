# 0.1.0: the cloud update

The owner asked (2026-11-17), word for word:

> For the cloud update let's make 0.1 let's add image and video gen section for those models, make it easy to use but not dumbed down

Decisions the owner made on 2026-11-17:

1. Companies in 0.1.0: **OpenRouter and OpenAI**, reusing the keys already saved in *Settings → Endpoints*.
2. **No picture tool for chat models** in 0.1.0. Chats and Studio connect through *Send to chat* and *Open in Studio*.
3. Spending (cloud runs): confirm every video, any run of $1 or more and any run with an unknown price, plus a **daily limit of $10, on by default**, that pauses Studio until it is raised.

Later the same day the owner added local generation:

> I also mean like downloading local models that can generate video.

and decided: local pictures and video ship in 0.1.0 with the cloud Studio, **local is the main thing and cloud is the backup** for computers without a good graphics card, people pick local models like Hugging Face downloads (with a free-for-any-use recommended list), and the Windows engine downloads with the first local model.

| File | What it is |
|---|---|
| [studio-design.md](studio-design.md) | The design: Studio, where it lives, what it shows, how runs, money, the gallery and privacy work, and the technical plan mapped to files |
| [studio-local-design.md](studio-local-design.md) | Addendum: local models in Studio (engine, downloads, fit, licences), with the owner's decisions at the top |
| [studio-plan.md](studio-plan.md) | The implementation plan, task by task |
| [research/providers.md](research/providers.md) | Image and video APIs as of 2026-11-17, with sources |
| [research/ux-patterns.md](research/ux-patterns.md) | How other generation tools stay easy without being dumbed down |
| [research/architecture-map.md](research/architecture-map.md) | Where Studio plugs into harnessUI today |
| research/local-*.md | Local engines and models, licences, and how the existing local runtime can be reused |
| research/catalog/ | OpenRouter's image and video catalogues as fetched on 2026-11-17 (test fixtures) |
