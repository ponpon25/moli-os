# Climatiseurs Hitachi (airCloud Go)

- Origine : l'API de l'application mobile *airCloud Go* (`api-global-prod.aircloudhome.com` pour le
  compte et les ordres, `notification-global-prod.aircloudhome.com` en WebSocket STOMP pour l'état).
  Rien n'est publié par Hitachi : le protocole a été relevé dans le trafic de l'application officielle
  (interception sur un téléphone du foyer), sans toucher aux appareils.
- **Passe par le cloud de Hitachi** : il faut internet, et le compte du foyer. Rien ne se passe en local ;
  si le cloud tombe, les clims restent pilotables à la télécommande et par l'appli… quand elle marche.
  (Une voie locale, en imitant le cloud Kii que le module appelle, a été éprouvée mais perd la confiance du
  module à chaque coupure de courant : voir `docs/components/pilotes.md`.)
- Découverte : les clims sont listées par le compte (message `ON_CONNECT` du service de notification) ;
  identité = nom du module (`JCH-…`), qui survit à un réappairage ; jamais d'identifiant codé en dur.
- État : message `ON_CONNECT` à la connexion, puis `REFRESH_ALL` demandé toutes les 60 s et 3 s / 10 s
  après un ordre. Le service ne pousse rien de lui-même dans ce qu'on a relevé.
- Ordres : `PUT /rac/basic-idu-control/general-control-command-status/{id}?familyId=…` avec **l'état
  complet** de la clim (le cloud n'accepte pas de delta) ; le pilote part du dernier état connu et ne change
  que le point demandé. Bornes (consigne, vitesses permises par mode) lues dans
  `rac-configuration` du compte.
- Points : `on`, `mode` (`froid`, `chaud`, `auto`, `déshumidification`, `ventilation`, comme Daikin :
  la carte « clim » du tableau de bord marche telle quelle), `target_temperature`, `temperature`,
  `fan_speed` (`auto`, `1`…`5`), `swing` (lecture seule : seule la valeur `OFF` a été vue), `fault`.
- Session : jeton de 30 minutes, renouvelé par une nouvelle connexion (l'application fait pareil ; aucun
  endpoint de rafraîchissement n'a été vu dans son trafic).
- `fixtures/` : exemples **fictifs** au format relevé (liste de clims, configuration des modèles), rejoués
  par les tests de `crates/moli-hitachi/src/model.rs`.
- Reste : minuteries et programmes hebdomadaires, consommation, lamelles (`swing`) en écriture, entretien
  (« frost wash »), mode vacances : des points d'accès existent, rien n'est relevé ni exposé.
