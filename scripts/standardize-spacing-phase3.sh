#!/bin/bash

# Phase 3 : Patterns restants (margin avec 4 valeurs, etc.)

TARGET_DIR="${1:-./frontend/src}"

echo "🔧 Phase 3 : Nettoyage final des patterns restants"
echo "===================================================="
echo ""

# Margin avec 4 valeurs (top right bottom left) - format: 0 0 Xpx 0
find "$TARGET_DIR" -type f \( -name "*.svelte" -o -name "*.css" \) -exec sed -i \
  -e 's/margin: 0 0 32px 0/margin: 0 0 var(--spacing-8) 0/g' \
  -e 's/margin: 0 0 24px 0/margin: 0 0 var(--spacing-6) 0/g' \
  -e 's/margin: 0 0 20px 0/margin: 0 0 var(--spacing-5) 0/g' \
  -e 's/margin: 0 0 16px 0/margin: 0 0 var(--spacing-4) 0/g' \
  -e 's/margin: 0 0 12px 0/margin: 0 0 var(--spacing-3) 0/g' \
  -e 's/margin: 0 0 8px 0/margin: 0 0 var(--spacing-2) 0/g' \
  -e 's/margin: 0 0 4px 0/margin: 0 0 var(--spacing-1) 0/g' \
  -e 's/margin: 0 0 2px 0/margin: 0 0 var(--spacing-0-5) 0/g' \
  -e 's/margin: 2px 0 0 0/margin: var(--spacing-0-5) 0 0 0/g' {} \;

# Margin avec 3 valeurs (top horizontal bottom) - format: 0 0 Xpx
find "$TARGET_DIR" -type f \( -name "*.svelte" -o -name "*.css" \) -exec sed -i \
  -e 's/margin: 0 0 32px/margin: 0 0 var(--spacing-8)/g' \
  -e 's/margin: 0 0 24px/margin: 0 0 var(--spacing-6)/g' \
  -e 's/margin: 0 0 20px/margin: 0 0 var(--spacing-5)/g' \
  -e 's/margin: 0 0 16px/margin: 0 0 var(--spacing-4)/g' \
  -e 's/margin: 0 0 12px/margin: 0 0 var(--spacing-3)/g' \
  -e 's/margin: 0 0 8px/margin: 0 0 var(--spacing-2)/g' \
  -e 's/margin: 0 0 4px/margin: 0 0 var(--spacing-1)/g' \
  -e 's/margin: 0 0 2px/margin: 0 0 var(--spacing-0-5)/g' {} \;

# Margin avec 2 valeurs - top 0
find "$TARGET_DIR" -type f \( -name "*.svelte" -o -name "*.css" \) -exec sed -i \
  -e 's/margin: 2px 0 0/margin: var(--spacing-0-5) 0 0/g' \
  -e 's/margin: 0 auto 24px/margin: 0 auto var(--spacing-6)/g' \
  -e 's/margin: 0 auto 16px/margin: 0 auto var(--spacing-4)/g' {} \;

# Padding avec valeurs atypiques
find "$TARGET_DIR" -type f \( -name "*.svelte" -o -name "*.css" \) -exec sed -i \
  -e 's/padding: 60px 20px/padding: var(--spacing-12) var(--spacing-5)/g' \
  -e 's/padding: 0 24px/padding: 0 var(--spacing-6)/g' \
  -e 's/padding: 0 20px/padding: 0 var(--spacing-5)/g' \
  -e 's/padding: 1px 7px/padding: 1px 7px/g' \
  -e 's/padding: 1px 6px/padding: 1px 6px/g' \
  -e 's/padding: 1px 4px/padding: 1px 4px/g' {} \;

echo "✅ Phase 3 terminée"
echo ""
echo "🔍 Vérification finale..."
REMAINING=$(grep -r "padding:\|margin:\|gap:" "$TARGET_DIR" --include="*.svelte" --include="*.css" | grep -E "[0-9]+(px|rem)" | grep -v "var(--" | grep -v "1px" | wc -l)
echo "  Valeurs hardcodées restantes (hors 1px): $REMAINING"
echo ""

if [ "$REMAINING" -eq 0 ]; then
  echo "🎉 SUCCÈS ! Tous les espacements sont standardisés !"
else
  echo "⚠️  Il reste quelques valeurs hardcodées. Voici les patterns restants:"
  grep -rh "padding:\|margin:\|gap:" "$TARGET_DIR" --include="*.svelte" --include="*.css" | grep -E "[0-9]+(px|rem)" | grep -v "var(--" | grep -v "1px" | sort | uniq | head -10
fi

echo ""
echo "📊 Résumé de la standardisation:"
echo "  - Phase 1: Valeurs simples (padding: Xpx, gap: Xpx)"
echo "  - Phase 2: Valeurs composées (padding: Xpx Ypx)"
echo "  - Phase 3: Patterns complexes (margin: 0 0 Xpx 0)"
echo ""
echo "🧹 Nettoyage des sauvegardes:"
echo "  find $TARGET_DIR -name '*.bak' -delete"
