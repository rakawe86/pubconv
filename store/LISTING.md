# Microsoft Store submission: PubConv

Everything Partner Center asks for, ready to paste. Screenshots are in `screenshots/` (1920×1080 PNG). The package is `PubConv-1.0.msix` from the GitHub Actions run (artifact `pubconv-packages`).

## What only the account owner can do
1. **Create the developer account** at https://storedeveloper.microsoft.com. Choose **Individual**, sign in with a personal Microsoft account, then verify with a selfie and government ID. It's free, and no card is needed.
2. **Reserve the name:** Partner Center → Apps and games → New product → **MSIX or PWA app** → name **PubConv**.
3. **Copy the three identity values:** Product → Product management → **Product identity**. Copy these into `packaging/release.json`:
   - `Package/Identity/Name` → `identity_name`
   - `Package/Identity/Publisher` → `publisher` (starts with `CN=`)
   - `Package/Properties/PublisherDisplayName` → `publisher_display_name`

After step 3, the package is rebuilt on GitHub with those values. Microsoft rejects a package whose identity doesn't match.

## Pricing and availability
- **Markets:** all.
- **Price:** Free. (Free apps need no payout or tax profile.)
- **Visibility:** public.

## Properties
- **Category:** Productivity. Subcategory if asked: none.
- **Privacy policy URL:** https://github.com/rakawe86/pubconv/blob/main/PRIVACY.md
- **Website:** https://github.com/rakawe86/pubconv
- **Third-party purchase API:** declare it. The Donate button opens the project page, which links to PayPal for voluntary donations that unlock nothing (Store Policy 10.8.2).
- **Support contact:** https://github.com/rakawe86/pubconv/issues
- **System requirements:** Windows 10 version 1809 or later, x64, 4 GB RAM; about 500 MB disk.

## Age rating questionnaire
Answer **No** to everything: no violence, no user communication, no purchases inside the app, no location, no personal data. The expected result is **3+ / Everyone**.

## Restricted capability justification (runFullTrust)
> PubConv is a desktop (Win32) application packaged with MSIX. It needs runFullTrust to run its conversion engine as a separate local process that reads the user's selected Publisher files and writes PDF files to folders the user chooses. It makes no network connections and collects no data.

## Store listing (English)

**Product name:** PubConv

**Short description** (shown on top; 1 sentence):
Convert Microsoft Publisher (.pub) files to PDF on your own PC, with WordArt and newsletter text kept intact.

**Description:**
Microsoft Publisher has reached end of support, and Microsoft recommends converting your .pub files to PDF. PubConv does it on your own computer, offline, without Publisher installed.

Most converters handle simple files but break on the parts Publisher users rely on most. WordArt headlines go missing or come out garbled. Newsletter articles that flow between linked text boxes get printed twice or cut off. PubConv reads those parts of the file properly, so your PDFs look the way the documents did in Publisher.

• Converts single files, a whole folder, or an entire archive with all its subfolders.
• Keeps WordArt headlines: curved, styled and outlined.
• Lays out newsletter articles across linked text boxes the way Publisher did.
• Text stays searchable and copyable in the PDF, including WordArt and newsletter text.
• Private: your files never leave your PC. No account, no upload, no internet needed.
• Saves PDFs next to the originals, or all in one folder you choose. Same-named files never overwrite each other.
• Never changes your original files.
• Includes a command line (pubconv-cli) for IT teams converting large archives.

Ideal for schools, churches, charities, small offices and anyone with years of Publisher newsletters, flyers, certificates and brochures to keep.

**What's new in this version:**
First release.

**Product features** (one per line):
- Converts Publisher (.pub) files to PDF offline
- Keeps WordArt headlines intact
- Lays out linked newsletter text the way Publisher did
- Converts whole folders and subfolders at once
- Searchable, copyable text in every PDF
- Files never leave your PC
- Never changes your original files
- Command line for bulk conversion

**Search terms** (up to 7):
Publisher to PDF · pub to pdf · Publisher converter · open pub file · Microsoft Publisher · pub file viewer · convert pub

**Screenshots** (in this order, with captions):
1. `1-app.png`: Convert Publisher files to PDF in one click
2. `2-wordart.png`: WordArt headlines kept
3. `3-newsletter.png`: Newsletters that read properly
4. `4-flyer.png`: Flyers and posters
5. `5-linked.png`: No repeated or lost text

**Store logos:** taken from the package automatically.

**Copyright:** © 2026 (publisher display name).

## Notes for certification
> PubConv converts Microsoft Publisher (.pub) files to PDF. To test: click "Add files…", choose any .pub file, then click "Convert to PDF". The PDF is saved next to the original. No account or internet connection is needed.
