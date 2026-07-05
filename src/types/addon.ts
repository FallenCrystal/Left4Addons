export interface AddonInfo {
  addonContent_Campaign?: string | number;
  addonContent_Map?: string | number;
  addonContent_Survivor?: string | number;
  addonContent_WeaponModel?: string | number;
  Content_WeaponModel?: string | number;
  Content_weapon?: string | number;
  addonContent_Skin?: string | number;
  addonContent_Script?: string | number;
  addonContent_Music?: string | number;
  addonContent_Sound?: string | number;
  addonContent_BossInfected?: string | number;
  addonContent_CommonInfected?: string | number;
  addonContent_UI?: string | number;
  addonContent_Spray?: string | number;
  addonContent_BackgroundMovie?: string | number;
  addonurl0?: string;
  addonURL0?: string;
  addonurl?: string;
  addonURL?: string;
  addonUrl0?: string;
  addonUrl?: string;
  addonauthor?: string;
  addonAuthor?: string;
  author?: string;
  addonauthorSteamID?: string;
  addonversion?: string;
  addontitle?: string;
  addondescription?: string;
  addonDescription?: string;
  addontagline?: string;
}

export interface SteamTag {
  tag: string;
}

export interface SteamDetails {
  title?: string;
  description?: string;
  creator_name?: string;
  creator?: string;
  tags?: SteamTag[];
}

export interface Addon {
  vpkName: string;
  dirType: 'workshop' | 'loading';
  isEnabled: boolean;
  fileSize: number;
  filesCount: number;
  imagePath?: string;
  workshopId?: string;
  addonInfo?: AddonInfo;
  steamDetails?: SteamDetails;
}

export interface Group {
  id: string;
  name: string;
  addons: string[];
}

export interface Settings {
  workshopDir: string;
  loadingDir: string;
}

export interface Toast {
  id: string;
  message: string;
  type: 'success' | 'error';
}
