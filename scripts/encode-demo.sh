#!/usr/bin/env bash
# Records scripts/record-demo.sh and encodes it three ways:
#   assets/demo.cast  the asciinema recording, replayable and diffable
#   assets/demo.gif   what the README embeds
#   assets/demo.mp4   the same thing, smaller and seekable
#
# Both tools are overridable, because neither ships in a distro repo:
#   asciinema   pip install asciinema
#   agg         github.com/asciinema/agg/releases
set -euo pipefail
cd "$(dirname "$0")/.."

for tool in "${ASCIINEMA:-asciinema}" "${AGG:-agg}" ffmpeg; do
  command -v "$tool" >/dev/null || { echo "missing: $tool" >&2; exit 1; }
done

ASCIINEMA=${ASCIINEMA:-asciinema}
AGG=${AGG:-agg}
COLS=${COLS:-120}
ROWS=${ROWS:-28}

mkdir -p assets

# SKIP_RECORD=1 re-encodes the existing cast. Encoding is where the fiddling
# happens -- theme, font, framerate -- and none of it needs the chain, so
# there is no reason to mint a new escrow every time a colour changes.
if [ "${SKIP_RECORD:-0}" = "1" ] && [ -f assets/demo.cast ]; then
  echo "reusing assets/demo.cast"
else
  rm -f assets/demo.cast
  # A real PTY at a fixed size, so the recording does not inherit whatever
  # window happened to be open.
  "$ASCIINEMA" rec assets/demo.cast \
    --command "stty cols $COLS rows $ROWS 2>/dev/null; ./scripts/record-demo.sh" \
    --cols "$COLS" --rows "$ROWS" \
    --idle-time-limit 3 \
    --overwrite --quiet
fi

# The project's own palette, so the GIF matches the screenshots in docs/.
# bg, fg, then all sixteen ANSI colours -- agg accepts 10 or 18 triplets and
# nothing in between. Same palette as the screenshots in docs/img.
THEME="0f0f0e,eceae5"                        # background, foreground
THEME="$THEME,1a1a18,cc5c4d,4ea36b,c8961e"   # black red green yellow
THEME="$THEME,5fa8b8,b48ead,7fc4d4,a3a099"   # blue magenta cyan white
THEME="$THEME,8b8880,e07a6a,6bbd88,ddaa3c"   # bright black red green yellow
THEME="$THEME,7fc4d4,c9a6c4,a5d8e2,eceae5"   # bright blue magenta cyan white

"$AGG" assets/demo.cast assets/demo.gif \
  --select "2%..100%" \
  --theme "$THEME" \
  --font-family "Adwaita Mono,JetBrains Mono,DejaVu Sans Mono" \
  --font-size 15 \
  --line-height 1.4 \
  --fps-cap 12 \
  --idle-time-limit 2.5 \
  --last-frame-duration 4

# From the GIF rather than the cast: agg only emits GIF, and re-encoding its
# frames keeps the two files identical rather than subtly different takes.
# yuv420p and the even-dimension pad are what make it play in browsers.
ffmpeg -y -loglevel error -i assets/demo.gif \
  -movflags +faststart -pix_fmt yuv420p \
  -vf "scale=trunc(iw/2)*2:trunc(ih/2)*2" \
  assets/demo.mp4

ls -lh assets/demo.cast assets/demo.gif assets/demo.mp4 | awk '{printf "  %-22s %s\n", $9, $5}'
