#!/bin/bash
set -euo pipefail

# Migration script for hardcoded values to CSS variables
# HomeVault Frontend Standardization

FRONTEND_DIR="/home/user/Projet/HOMEVAULT - HTMX/frontend/src"
BACKUP_DIR="/home/user/Projet/HOMEVAULT - HTMX/.backup-$(date +%Y%m%d-%H%M%S)"

echo "🚀 Démarrage de la migration des valeurs hardcodées"
echo "📁 Répertoire: $FRONTEND_DIR"
echo ""

# Create a backup
echo "💾 Création du backup dans $BACKUP_DIR"
mkdir -p "$BACKUP_DIR"
cp -r "$FRONTEND_DIR" "$BACKUP_DIR/"
echo "✅ Backup créé"
echo ""

cd "$FRONTEND_DIR"

# Compteurs
TOTAL_REPLACEMENTS=0

echo "🎨 Migration des couleurs..."

# Couleurs de bordure
find . -name "*.svelte" -type f -exec sed -i 's/#e5e7eb/var(--color-border)/g' {} +
find . -name "*.svelte" -type f -exec sed -i "s/'#e5e7eb'/var(--color-border)/g" {} +
find . -name "*.svelte" -type f -exec sed -i 's/"#e5e7eb"/var(--color-border)/g' {} +
TOTAL_REPLACEMENTS=$((TOTAL_REPLACEMENTS + $(grep -r "var(--color-border)" . --include="*.svelte" | wc -l)))

# Couleurs de fond neutral
find . -name "*.svelte" -type f -exec sed -i 's/#f9fafb/var(--color-neutral-50)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/#f3f4f6/var(--color-neutral-100)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/#e5e7eb/var(--color-neutral-200)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/#d1d5db/var(--color-neutral-300)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/#9ca3af/var(--color-neutral-400)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/#6b7280/var(--color-neutral-500)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/#4b5563/var(--color-neutral-600)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/#374151/var(--color-neutral-700)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/#1f2937/var(--color-neutral-800)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/#111827/var(--color-neutral-900)/g' {} +

# Couleurs de texte
find . -name "*.svelte" -type f -exec sed -i 's/#64748b/var(--color-text-muted)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/#94a3b8/var(--color-text-secondary)/g' {} +

echo "✅ Couleurs migrées"
echo ""

echo "📏 Migration des espacements..."

# Paddings
find . -name "*.svelte" -type f -exec sed -i 's/padding: 4px/padding: var(--spacing-1)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/padding: 8px/padding: var(--spacing-2)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/padding: 12px/padding: var(--spacing-3)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/padding: 16px/padding: var(--spacing-4)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/padding: 20px/padding: var(--spacing-5)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/padding: 24px/padding: var(--spacing-6)/g' {} +

# Compound paddings
find . -name "*.svelte" -type f -exec sed -i 's/padding: 8px 12px/padding: var(--spacing-2) var(--spacing-3)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/padding: 12px 16px/padding: var(--spacing-3) var(--spacing-4)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/padding: 16px 20px/padding: var(--spacing-4) var(--spacing-5)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/padding: 20px 24px/padding: var(--spacing-5) var(--spacing-6)/g' {} +

# Gaps
find . -name "*.svelte" -type f -exec sed -i 's/gap: 4px/gap: var(--spacing-1)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/gap: 8px/gap: var(--spacing-2)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/gap: 12px/gap: var(--spacing-3)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/gap: 16px/gap: var(--spacing-4)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/gap: 20px/gap: var(--spacing-5)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/gap: 24px/gap: var(--spacing-6)/g' {} +

echo "✅ Espacements migrés"
echo ""

echo "🔲 Migration des bordures..."

# Bordures
find . -name "*.svelte" -type f -exec sed -i 's/border: 1px solid #e5e7eb/border: 1px solid var(--color-border)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/border-bottom: 1px solid #e5e7eb/border-bottom: 1px solid var(--color-border)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/border-top: 1px solid #e5e7eb/border-top: 1px solid var(--color-border)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/border-left: 1px solid #e5e7eb/border-left: 1px solid var(--color-border)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/border-right: 1px solid #e5e7eb/border-right: 1px solid var(--color-border)/g' {} +

# Border radius
find . -name "*.svelte" -type f -exec sed -i 's/border-radius: 4px/border-radius: var(--radius-sm)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/border-radius: 6px/border-radius: var(--radius-base)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/border-radius: 8px/border-radius: var(--radius-md)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/border-radius: 12px/border-radius: var(--radius-lg)/g' {} +

echo "✅ Bordures migrées"
echo ""

echo "📝 Migration des tailles de police..."

# Font sizes
find . -name "*.svelte" -type f -exec sed -i 's/font-size: 11px/font-size: var(--text-xs)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/font-size: 13px/font-size: var(--text-sm)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/font-size: 14px/font-size: var(--text-base)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/font-size: 16px/font-size: var(--text-lg)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/font-size: 18px/font-size: var(--text-xl)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/font-size: 20px/font-size: var(--text-2xl)/g' {} +

echo "✅ Tailles de police migrées"
echo ""

echo "⏱️  Migration des transitions..."

# Transitions
find . -name "*.svelte" -type f -exec sed -i 's/transition: all 150ms/transition: all var(--transition-fast)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/transition: all 200ms/transition: all var(--transition-base)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/transition: all 300ms/transition: all var(--transition-slow)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/transition: all 0.15s/transition: all var(--transition-fast)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/transition: all 0.2s/transition: all var(--transition-base)/g' {} +
find . -name "*.svelte" -type f -exec sed -i 's/transition: all 0.3s/transition: all var(--transition-slow)/g' {} +

echo "✅ Transitions migrées"
echo ""

echo "🔍 Rapport de migration:"
echo ""

# Compter les occurrences
COLOR_VARS=$(grep -r "var(--color-" . --include="*.svelte" | wc -l)
SPACING_VARS=$(grep -r "var(--spacing-" . --include="*.svelte" | wc -l)
TEXT_VARS=$(grep -r "var(--text-" . --include="*.svelte" | wc -l)
RADIUS_VARS=$(grep -r "var(--radius-" . --include="*.svelte" | wc -l)
TRANSITION_VARS=$(grep -r "var(--transition-" . --include="*.svelte" | wc -l)

echo "📊 Variables CSS utilisées:"
echo "   - Couleurs: $COLOR_VARS occurrences"
echo "   - Espacements: $SPACING_VARS occurrences"
echo "   - Texte: $TEXT_VARS occurrences"
echo "   - Radius: $RADIUS_VARS occurrences"
echo "   - Transitions: $TRANSITION_VARS occurrences"
echo ""

TOTAL_VARS=$((COLOR_VARS + SPACING_VARS + TEXT_VARS + RADIUS_VARS + TRANSITION_VARS))
echo "✅ Total: $TOTAL_VARS variables CSS"
echo ""

echo "⚠️  Vérification manuelle requise pour:"
echo "   - frontend/src/routes/Login.svelte (gradient custom)"
echo "   - frontend/src/routes/Dashboard.svelte (grid layouts)"
echo "   - frontend/src/routes/Explorer.svelte (largeurs sidebar)"
echo ""

echo "💾 Backup disponible dans: $BACKUP_DIR"
echo ""
echo "✅ Migration terminée avec succès!"
