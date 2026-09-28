#!/usr/bin/env node

import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const srcDir = path.join(__dirname, '../src');

function scanDirectory(dir, iconSet = new Set()) {
  const files = fs.readdirSync(dir, { withFileTypes: true });

  for (const file of files) {
    const fullPath = path.join(dir, file.name);

    if (file.isDirectory()) {
      scanDirectory(fullPath, iconSet);
    } else if (file.name.endsWith('.svelte') || file.name.endsWith('.ts')) {
      const content = fs.readFileSync(fullPath, 'utf-8');
      
      // Match Icon name="..." patterns
      const iconMatches = content.match(/Icon name="([^"]*)"/g);
      if (iconMatches) {
        iconMatches.forEach(match => {
          const name = match.match(/Icon name="([^"]*)"/)[1];
          iconSet.add(name);
        });
      }

      // Match PapirusIcon name="..." patterns
      const papirusMatches = content.match(/PapirusIcon name="([^"]*)"/g);
      if (papirusMatches) {
        papirusMatches.forEach(match => {
          const name = match.match(/PapirusIcon name="([^"]*)"/)[1];
          iconSet.add(`papirus-${name}`);
        });
      }
    }
  }

  return iconSet;
}

const allIcons = scanDirectory(srcDir);
const iconArray = Array.from(allIcons).sort();

console.log(`Total unique icons found: ${iconArray.length}`);
console.log('\nAll icons used:');
iconArray.forEach(icon => console.log(`  - ${icon}`));

// Save to file
fs.writeFileSync(
  path.join(__dirname, '../all-icons.txt'),
  iconArray.join('\n')
);
console.log('\n✅ Saved to all-icons.txt');
