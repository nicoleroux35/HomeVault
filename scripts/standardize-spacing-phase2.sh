#!/bin/bash

# Phase 2 : Patterns complexes (padding/margin avec 2-4 valeurs)

TARGET_DIR="${1:-./frontend/src}"

echo "🔧 Phase 2 : Standardisation des patterns complexes"
echo "===================================================="
echo ""

# Padding avec 2 valeurs (vertical horizontal)
sed -i 's/padding: 16px 24px/padding: var(--spacing-4) var(--spacing-6)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 12px 16px/padding: var(--spacing-3) var(--spacing-4)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 8px 16px/padding: var(--spacing-2) var(--spacing-4)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 8px 12px/padding: var(--spacing-2) var(--spacing-3)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 6px 12px/padding: var(--spacing-1-5) var(--spacing-3)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 4px 8px/padding: var(--spacing-1) var(--spacing-2)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 2px 8px/padding: var(--spacing-0-5) var(--spacing-2)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 2px 6px/padding: var(--spacing-0-5) var(--spacing-1-5)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")

# Padding avec 3 valeurs (top horizontal bottom)
sed -i 's/padding: 16px 24px 16px/padding: var(--spacing-4) var(--spacing-6)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 12px 16px 12px/padding: var(--spacing-3) var(--spacing-4)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")

# Padding avec 4 valeurs (top right bottom left)
sed -i 's/padding: 16px 24px 16px 24px/padding: var(--spacing-4) var(--spacing-6)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 12px 16px 12px 16px/padding: var(--spacing-3) var(--spacing-4)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")

# Margin avec 2 valeurs
sed -i 's/margin: 0 auto/margin: 0 auto/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")  # Keep as is
sed -i 's/margin: 16px 0/margin: var(--spacing-4) 0/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin: 8px 0/margin: var(--spacing-2) 0/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin: 0 16px/margin: 0 var(--spacing-4)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin: 0 12px/margin: 0 var(--spacing-3)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")

# Margin bottom/top courants
sed -i 's/margin-bottom: 24px/margin-bottom: var(--spacing-6)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin-bottom: 20px/margin-bottom: var(--spacing-5)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin-bottom: 12px/margin-bottom: var(--spacing-3)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin-bottom: 8px/margin-bottom: var(--spacing-2)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin-bottom: 4px/margin-bottom: var(--spacing-1)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")

sed -i 's/margin-top: 24px/margin-top: var(--spacing-6)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin-top: 20px/margin-top: var(--spacing-5)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin-top: 12px/margin-top: var(--spacing-3)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin-top: 8px/margin-top: var(--spacing-2)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")

# Padding top/bottom/left/right
sed -i 's/padding-top: 24px/padding-top: var(--spacing-6)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding-top: 20px/padding-top: var(--spacing-5)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding-top: 12px/padding-top: var(--spacing-3)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding-top: 8px/padding-top: var(--spacing-2)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")

sed -i 's/padding-bottom: 24px/padding-bottom: var(--spacing-6)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding-bottom: 20px/padding-bottom: var(--spacing-5)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding-bottom: 12px/padding-bottom: var(--spacing-3)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding-bottom: 8px/padding-bottom: var(--spacing-2)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")

sed -i 's/padding-left: 24px/padding-left: var(--spacing-6)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding-left: 20px/padding-left: var(--spacing-5)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding-left: 12px/padding-left: var(--spacing-3)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding-left: 8px/padding-left: var(--spacing-2)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")

sed -i 's/padding-right: 24px/padding-right: var(--spacing-6)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding-right: 20px/padding-right: var(--spacing-5)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding-right: 12px/padding-right: var(--spacing-3)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding-right: 8px/padding-right: var(--spacing-2)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")

# Valeurs rem (SetupWizard, etc.)
sed -i 's/padding: 2rem/padding: var(--spacing-8)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 2\.5rem/padding: var(--spacing-10)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 1\.5rem/padding: var(--spacing-6)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 1rem/padding: var(--spacing-4)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 0\.75rem/padding: var(--spacing-3)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 0\.5rem/padding: var(--spacing-2)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")

sed -i 's/margin: 1\.5rem/margin: var(--spacing-6)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin: 1rem/margin: var(--spacing-4)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin: 0\.5rem/margin: var(--spacing-2)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin: 0\.75rem/margin: var(--spacing-3)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin: 0\.25rem/margin: var(--spacing-1)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")

sed -i 's/gap: 1\.5rem/gap: var(--spacing-6)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/gap: 1rem/gap: var(--spacing-4)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/gap: 0\.75rem/gap: var(--spacing-3)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/gap: 0\.5rem/gap: var(--spacing-2)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/gap: 0\.4rem/gap: var(--spacing-2)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/gap: 0\.25rem/gap: var(--spacing-1)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")

# Margin avec rem et valeurs multiples
sed -i 's/margin: 0 0 1\.5rem 0/margin: 0 0 var(--spacing-6) 0/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin: 0 0 1rem 0/margin: 0 0 var(--spacing-4) 0/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin: 0 0 0\.75rem 0/margin: 0 0 var(--spacing-3) 0/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin: 0 0 0\.5rem 0/margin: 0 0 var(--spacing-2) 0/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/margin: 0 auto 1\.5rem/margin: 0 auto var(--spacing-6)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")

# Padding avec rem et valeurs multiples
sed -i 's/padding: 0\.75rem 1rem/padding: var(--spacing-3) var(--spacing-4)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 0\.75rem 1\.5rem/padding: var(--spacing-3) var(--spacing-6)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 0\.4rem 0\.75rem/padding: var(--spacing-2) var(--spacing-3)/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")
sed -i 's/padding: 0\.5rem 0/padding: var(--spacing-2) 0/g' $(find "$TARGET_DIR" -name "*.svelte" -o -name "*.css")

echo "✅ Phase 2 terminée"
echo ""
echo "🔍 Vérification des valeurs restantes..."
REMAINING=$(grep -r "padding:\|margin:\|gap:" "$TARGET_DIR" --include="*.svelte" --include="*.css" | grep -E "[0-9]+(px|rem)" | grep -v "var(--" | wc -l)
echo "  Valeurs hardcodées restantes: $REMAINING"
echo ""
