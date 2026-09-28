#!/bin/bash

# Phase 2 migration tracking script
echo "📊 PROGRESSION MIGRATIONS PHASE 2"
echo "════════════════════════════════════════════════════════"
echo ""

cd "/home/user/Projet/HOMEVAULT - HTMX/frontend/src/routes"

# Pages to migrate
PAGES=("Settings.svelte" "Groups.svelte" "Users.svelte" "Dashboard.svelte" "Explorer.svelte")
TOTAL=${#PAGES[@]}
MIGRATED=0

echo "✅ État des migrations Layout:"
echo "────────────────────────────────────────────────────────"

for page in "${PAGES[@]}"; do
  if [ -f "$page" ]; then
    if grep -q "from '\$lib/components/layout'" "$page" 2>/dev/null; then
      echo "✅ $page - Migré vers Layout"
      MIGRATED=$((MIGRATED + 1))
    else
      echo "⏳ $page - À migrer"
    fi
  else
    echo "❌ $page - Fichier non trouvé"
  fi
done

echo ""
echo "────────────────────────────────────────────────────────"
PERCENT=$((MIGRATED * 100 / TOTAL))
echo "Progression: $MIGRATED/$TOTAL pages migrées ($PERCENT%)"
echo ""

if [ $MIGRATED -eq $TOTAL ]; then
  echo "🎉 TOUTES LES PAGES SONT MIGRÉES!"
  echo ""
  echo "✅ Phase 2 - Migrations Layout: 100%"
  echo "✅ Phase 2 globale: 100%"
else
  REMAINING=$((TOTAL - MIGRATED))
  TIME_LEFT=$((REMAINING * 20))
  echo "⏱️  Temps estimé restant: ~$TIME_LEFT minutes"
  echo "📈 Phase 2 - Migrations: ${PERCENT}%"
fi

echo ""
