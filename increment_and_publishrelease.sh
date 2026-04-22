#!/bin/bash
VERSION_FILE="version.txt"
if [ ! -f "$VERSION_FILE" ]; then
    echo "0.$(date +%y%m%d).0" > "$VERSION_FILE"
fi

CURRENT_VERSION=$(cat $VERSION_FILE)
MAJOR=$(echo $CURRENT_VERSION | cut -d. -f1)
OLD_DATE=$(echo $CURRENT_VERSION | cut -d. -f2)
MINOR=$(echo $CURRENT_VERSION | cut -d. -f3)

NEW_DATE=$(date +%y%m%d)

if [ "$OLD_DATE" == "$NEW_DATE" ]; then
    NEW_MINOR=$((MINOR + 1))
else
    NEW_MINOR=1
fi

NEW_VERSION="$MAJOR.$NEW_DATE.$NEW_MINOR"
echo $NEW_VERSION > $VERSION_FILE

git add .
git commit -m "Release $NEW_VERSION"
git tag -a "v$NEW_VERSION" -m "Version $NEW_VERSION"
git push origin master
git push origin "v$NEW_VERSION"
