# WiZ

- API UDP locale (port 38899), JSON sans clé ni compte : `getSystemConfig`, `getPilot`, `setPilot`.
- Pas de fixture : les tests lisent des réponses `getPilot` écrites dans le code.
- Cible : un bandeau LED couleur (`ESP03_SHRGB1W_01`).
- Reste : scènes WiZ non exposées ; pas de découverte (adresse fixe à réserver sur la box).
