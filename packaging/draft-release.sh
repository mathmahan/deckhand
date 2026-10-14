#!/bin/bash
# Attach built files to the Forgejo release for a tag, creating it as a draft if there is none, and
# give each file's SHA-256 in its notes.
#
# The app's own updater (src/release.rs) installs an AppImage only when the notes give its SHA-256
# on a line naming it, and shows the notes' first line as what the release is about. A draft is
# never offered, so a new release waits as one, its first line a placeholder, until someone writes
# that line and publishes it. Run again for the same tag, the files and their checksum lines are
# replaced and the rest of the notes is kept; a release already published stays published.
#
# Usage: packaging/draft-release.sh <tag> <file>...
# Needs curl, jq and sha256sum, and FORGEJO_API_URL, FORGEJO_REPOSITORY and FORGEJO_TOKEN in the
# environment — a Forgejo Actions job has all three, and its own token may write its releases.
set -euo pipefail

TAG="$1"
shift
API="$FORGEJO_API_URL/repos/$FORGEJO_REPOSITORY"

# Forgejo's own words on a refusal go to the log rather than into the variable being read.
api() {
    local out
    if ! out="$(curl -sS --fail-with-body -H "Authorization: token $FORGEJO_TOKEN" "$@")"; then
        printf '%s\n' "$out" >&2
        return 1
    fi
    printf '%s' "$out"
}

# The list has drafts in it as well, which a lookup by tag may not.
RELEASE="$(api "$API/releases?limit=50" | jq -c --arg tag "$TAG" 'map(select(.tag_name == $tag)) | first // empty')"

NAMES="$(for file in "$@"; do basename "$file"; done)"
NOTES="$(
    if [ -n "$RELEASE" ]; then
        jq -r '.body // ""' <<<"$RELEASE"
    else
        printf '**New in %s: …**\n' "${TAG#v}"
    fi | awk -v names="$NAMES" '
        BEGIN {
            n = split(names, name, "\n")
            hex = "[0-9a-fA-F]"
            for (i = 0; i < 6; i++) hex = hex hex
        }
        $0 ~ hex { for (i = 1; i <= n; i++) if (index($0, name[i])) next }
        /^[[:space:]]*$/ { blank++; next }
        { for (; blank > 0; blank--) print ""; print }'
    echo
    for file in "$@"; do
        printf 'sha256 `%s` %s\n' "$(sha256sum "$file" | cut -d' ' -f1)" "$(basename "$file")"
    done
)"

if [ -z "$RELEASE" ]; then
    RELEASE="$(jq -n --arg tag "$TAG" --arg body "$NOTES" '{tag_name: $tag, name: "Deckhand \($tag | ltrimstr("v"))", body: $body, draft: true}' |
        api -X POST -H 'Content-Type: application/json' -d @- "$API/releases")"
else
    RELEASE="$(jq -n --arg body "$NOTES" '{body: $body}' |
        api -X PATCH -H 'Content-Type: application/json' -d @- "$API/releases/$(jq -r .id <<<"$RELEASE")")"
fi
ID="$(jq -r .id <<<"$RELEASE")"

for file in "$@"; do
    name="$(basename "$file")"
    for old in $(api "$API/releases/$ID/assets" | jq -r --arg name "$name" '.[] | select(.name == $name) | .id'); do
        api -X DELETE "$API/releases/$ID/assets/$old"
    done
    echo "==> Uploading $name"
    api -X POST -F "attachment=@$file" "$API/releases/$ID/assets?name=$name" >/dev/null
done

if [ "$(jq -r .draft <<<"$RELEASE")" = true ]; then
    echo "==> Draft $TAG: write its first line and publish it at $(jq -r .html_url <<<"$RELEASE")"
else
    echo "==> $TAG: $(jq -r .html_url <<<"$RELEASE")"
fi
