# Sound Files

This directory contains notification and UI sound effects.

## Required Files

Place the following MP3 files in this directory:

| File | Description | Source Reference |
|------|-------------|-----------------|
| `notification.mp3` | General notification sound | universfield-new-notification-040-493469.mp3 |
| `login-success.mp3` | Login successful sound | universfield-new-notification-059-494262.mp3 |
| `login-error.mp3` | Login failed sound | universfield-new-notification-042-493470.mp3 |

## Usage

Sounds are automatically initialized by `app.js` on page load.

```javascript
// Play notification sound
Sound.play('notification');

// Play login sounds (called automatically by login-page.js)
Sound.play('loginSuccess');
Sound.play('loginError');
```

## Recommendations

- Keep sounds short (< 1 second)
- Use MP3 format for compatibility
- Recommended volume: normalize to -3dB
