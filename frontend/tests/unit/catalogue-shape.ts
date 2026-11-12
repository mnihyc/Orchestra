/** Ordinary catalogue values include nested objects and ordered keyword lists. */
export interface Catalogue {
  [key: string]: CatalogueValue;
}
export type CatalogueValue = string | Catalogue | CatalogueValue[];
export type CatalogueBranch = Catalogue | CatalogueValue[];
