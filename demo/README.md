# CLI demo

`notypo-demo.avif` is the primary GitHub README animation, with animated WebP
and GIF fallbacks.

Regenerate all three files with:

```sh
brew install vhs ffmpeg webp
./scripts/record-demo.sh
```

The recording uses zsh because `t` depends on interactive shell history.
The **Demo** GitHub Actions workflow runs the same script on demand and uploads
the three assets.

The tape starts a fresh zsh session, initializes a temporary git repository,
loads the notypo aliases, then shows `git sttus` corrected with `t`.
